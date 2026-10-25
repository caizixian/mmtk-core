#![deny(unsafe_op_in_unsafe_fn)]

use std::mem::{offset_of, size_of, MaybeUninit};
use std::sync::Arc;

use crate::policy::largeobjectspace::LargeObjectSpace;
use crate::policy::marksweepspace::malloc_ms::MallocSpace;
use crate::policy::marksweepspace::native_ms::MarkSweepSpace;
use crate::policy::space::Space;
use crate::util::alloc::LargeObjectAllocator;
use crate::util::alloc::MallocAllocator;
use crate::util::alloc::{Allocator, BumpAllocator, ImmixAllocator};
use crate::util::VMMutatorThread;
use crate::vm::VMBinding;
use crate::Mutator;
use crate::MMTK;

use super::allocator::AllocatorContext;
use super::FreeListAllocator;
use super::Lisp2Allocator;

pub(crate) const MAX_BUMP_ALLOCATORS: usize = 6;
pub(crate) const MAX_LARGE_OBJECT_ALLOCATORS: usize = 2;
pub(crate) const MAX_MALLOC_ALLOCATORS: usize = 1;
pub(crate) const MAX_IMMIX_ALLOCATORS: usize = 2;
pub(crate) const MAX_FREE_LIST_ALLOCATORS: usize = 2;
pub(crate) const MAX_LISP2_ALLOCATORS: usize = 1;

const _: () = assert!(
    MAX_BUMP_ALLOCATORS
        + MAX_LARGE_OBJECT_ALLOCATORS
        + MAX_MALLOC_ALLOCATORS
        + MAX_IMMIX_ALLOCATORS
        + MAX_FREE_LIST_ALLOCATORS
        + MAX_LISP2_ALLOCATORS
        <= u16::BITS as usize
);

/// Compact `#[repr(transparent)]` bitset recording which allocator slots in [`Allocators`] have
/// been initialized (`14 <= 16` total slots across all 6 allocator families).
#[repr(transparent)]
#[derive(Copy, Clone, Default, Debug, Eq, PartialEq)]
struct InitSelectorSet(u16);

impl InitSelectorSet {
    const BASE_BUMP: usize = 0;
    const BASE_LARGE_OBJECT: usize = Self::BASE_BUMP + MAX_BUMP_ALLOCATORS;
    const BASE_MALLOC: usize = Self::BASE_LARGE_OBJECT + MAX_LARGE_OBJECT_ALLOCATORS;
    const BASE_IMMIX: usize = Self::BASE_MALLOC + MAX_MALLOC_ALLOCATORS;
    const BASE_FREE_LIST: usize = Self::BASE_IMMIX + MAX_IMMIX_ALLOCATORS;
    const BASE_LISP2: usize = Self::BASE_FREE_LIST + MAX_FREE_LIST_ALLOCATORS;

    const ALL_SELECTORS: [AllocatorSelector; 14] = [
        AllocatorSelector::BumpPointer(0),
        AllocatorSelector::BumpPointer(1),
        AllocatorSelector::BumpPointer(2),
        AllocatorSelector::BumpPointer(3),
        AllocatorSelector::BumpPointer(4),
        AllocatorSelector::BumpPointer(5),
        AllocatorSelector::LargeObject(0),
        AllocatorSelector::LargeObject(1),
        AllocatorSelector::Malloc(0),
        AllocatorSelector::Immix(0),
        AllocatorSelector::Immix(1),
        AllocatorSelector::FreeList(0),
        AllocatorSelector::FreeList(1),
        AllocatorSelector::Lisp2(0),
    ];

    #[inline(always)]
    const fn bit_index(selector: AllocatorSelector) -> Option<u32> {
        let bit = match selector {
            AllocatorSelector::BumpPointer(i) if (i as usize) < MAX_BUMP_ALLOCATORS => {
                Self::BASE_BUMP + i as usize
            }
            AllocatorSelector::LargeObject(i) if (i as usize) < MAX_LARGE_OBJECT_ALLOCATORS => {
                Self::BASE_LARGE_OBJECT + i as usize
            }
            AllocatorSelector::Malloc(i) if (i as usize) < MAX_MALLOC_ALLOCATORS => {
                Self::BASE_MALLOC + i as usize
            }
            AllocatorSelector::Immix(i) if (i as usize) < MAX_IMMIX_ALLOCATORS => {
                Self::BASE_IMMIX + i as usize
            }
            AllocatorSelector::FreeList(i) if (i as usize) < MAX_FREE_LIST_ALLOCATORS => {
                Self::BASE_FREE_LIST + i as usize
            }
            AllocatorSelector::Lisp2(i) if (i as usize) < MAX_LISP2_ALLOCATORS => {
                Self::BASE_LISP2 + i as usize
            }
            _ => return None,
        };
        Some(bit as u32)
    }

    #[inline(always)]
    const fn contains(self, selector: AllocatorSelector) -> bool {
        match Self::bit_index(selector) {
            Some(bit) => (self.0 & (1u16 << bit)) != 0,
            None => false,
        }
    }

    #[inline(always)]
    fn insert(&mut self, selector: AllocatorSelector) {
        let bit = Self::bit_index(selector)
            .unwrap_or_else(|| panic!("Out-of-bounds or invalid AllocatorSelector: {selector:?}"));
        self.0 |= 1u16 << bit;
    }
}

/// The allocator set owned by each [`Mutator`].
///
/// # `#[repr(C)]` Layout Note
/// All 6 `[MaybeUninit<...>; MAX_*]` arrays are module-private and followed by a trailing
/// `InitSelectorSet(u16)` (`2` bytes plus `6` bytes of trailing pointer-alignment padding = `+8`
/// bytes at the end of `Allocators<VM>`). Fast-path bump/immix/lisp2 allocator offsets at the
/// start of `Mutator<VM>` remain fixed-size and `#[repr(C)]`, while fields of [`Mutator`] after
/// `allocators` (`barrier`, `mutator_tls`, etc.) include this trailing `8`-byte metadata word.
/// VM bindings should query offsets via [`Mutator::get_allocator_base_offset`] or
/// [`AllocatorInfo::new`] rather than hardcoding byte sizes.
#[repr(C)]
pub struct Allocators<VM: VMBinding> {
    bump_pointer: [MaybeUninit<BumpAllocator<VM>>; MAX_BUMP_ALLOCATORS],
    large_object: [MaybeUninit<LargeObjectAllocator<VM>>; MAX_LARGE_OBJECT_ALLOCATORS],
    malloc: [MaybeUninit<MallocAllocator<VM>>; MAX_MALLOC_ALLOCATORS],
    immix: [MaybeUninit<ImmixAllocator<VM>>; MAX_IMMIX_ALLOCATORS],
    free_list: [MaybeUninit<FreeListAllocator<VM>>; MAX_FREE_LIST_ALLOCATORS],
    lisp2: [MaybeUninit<Lisp2Allocator<VM>>; MAX_LISP2_ALLOCATORS],
    init_set: InitSelectorSet,
}

impl<VM: VMBinding> Drop for Allocators<VM> {
    fn drop(&mut self) {
        for i in 0..(MAX_BUMP_ALLOCATORS as u8) {
            if self.init_set.contains(AllocatorSelector::BumpPointer(i)) {
                // SAFETY: `self.init_set` records slots initialized via `MaybeUninit::write` in
                // `Allocators::new`; no other method calls `assume_init_drop` or overwrites slots
                // with uninitialized data, and `Drop::drop` executes at most once.
                unsafe { self.bump_pointer[i as usize].assume_init_drop() };
            }
        }
        for i in 0..(MAX_LARGE_OBJECT_ALLOCATORS as u8) {
            if self.init_set.contains(AllocatorSelector::LargeObject(i)) {
                // SAFETY: Same module-sealed initialization invariant for `LargeObjectAllocator<VM>`.
                unsafe { self.large_object[i as usize].assume_init_drop() };
            }
        }
        for i in 0..(MAX_MALLOC_ALLOCATORS as u8) {
            if self.init_set.contains(AllocatorSelector::Malloc(i)) {
                // SAFETY: Same module-sealed initialization invariant for `MallocAllocator<VM>`.
                unsafe { self.malloc[i as usize].assume_init_drop() };
            }
        }
        for i in 0..(MAX_IMMIX_ALLOCATORS as u8) {
            if self.init_set.contains(AllocatorSelector::Immix(i)) {
                // SAFETY: Same module-sealed initialization invariant for `ImmixAllocator<VM>`.
                unsafe { self.immix[i as usize].assume_init_drop() };
            }
        }
        for i in 0..(MAX_FREE_LIST_ALLOCATORS as u8) {
            if self.init_set.contains(AllocatorSelector::FreeList(i)) {
                // SAFETY: Same module-sealed initialization invariant for `FreeListAllocator<VM>`.
                unsafe { self.free_list[i as usize].assume_init_drop() };
            }
        }
        for i in 0..(MAX_LISP2_ALLOCATORS as u8) {
            if self.init_set.contains(AllocatorSelector::Lisp2(i)) {
                // SAFETY: Same module-sealed initialization invariant for `Lisp2Allocator<VM>`.
                unsafe { self.lisp2[i as usize].assume_init_drop() };
            }
        }
    }
}

impl<VM: VMBinding> Allocators<VM> {
    /// Return the byte offset within `Allocators<VM>` of the allocator slot designated by `selector`.
    pub(crate) const fn field_offset(selector: AllocatorSelector) -> usize {
        match selector {
            AllocatorSelector::BumpPointer(index) => {
                assert!(
                    (index as usize) < MAX_BUMP_ALLOCATORS,
                    "BumpPointer selector index out of bounds"
                );
                offset_of!(Self, bump_pointer) + size_of::<BumpAllocator<VM>>() * index as usize
            }
            AllocatorSelector::LargeObject(index) => {
                assert!(
                    (index as usize) < MAX_LARGE_OBJECT_ALLOCATORS,
                    "LargeObject selector index out of bounds"
                );
                offset_of!(Self, large_object)
                    + size_of::<LargeObjectAllocator<VM>>() * index as usize
            }
            AllocatorSelector::Malloc(index) => {
                assert!(
                    (index as usize) < MAX_MALLOC_ALLOCATORS,
                    "Malloc selector index out of bounds"
                );
                offset_of!(Self, malloc) + size_of::<MallocAllocator<VM>>() * index as usize
            }
            AllocatorSelector::Immix(index) => {
                assert!(
                    (index as usize) < MAX_IMMIX_ALLOCATORS,
                    "Immix selector index out of bounds"
                );
                offset_of!(Self, immix) + size_of::<ImmixAllocator<VM>>() * index as usize
            }
            AllocatorSelector::FreeList(index) => {
                assert!(
                    (index as usize) < MAX_FREE_LIST_ALLOCATORS,
                    "FreeList selector index out of bounds"
                );
                offset_of!(Self, free_list) + size_of::<FreeListAllocator<VM>>() * index as usize
            }
            AllocatorSelector::Lisp2(index) => {
                assert!(
                    (index as usize) < MAX_LISP2_ALLOCATORS,
                    "Lisp2 selector index out of bounds"
                );
                offset_of!(Self, lisp2) + size_of::<Lisp2Allocator<VM>>() * index as usize
            }
            AllocatorSelector::None => panic!("Expect a valid AllocatorSelector, found None"),
        }
    }

    /// Return a reference to the initialized allocator selected by `selector`.
    /// Panics if `selector` is `AllocatorSelector::None`, out of bounds, or was not initialized.
    pub(crate) fn get_allocator(&self, selector: AllocatorSelector) -> &dyn Allocator<VM> {
        if selector == AllocatorSelector::None {
            panic!("Allocator mapping is not initialized");
        }
        assert!(
            self.init_set.contains(selector),
            "Allocator slot {selector:?} is not initialized"
        );
        match selector {
            AllocatorSelector::BumpPointer(index) => {
                let slot = &self.bump_pointer[index as usize];
                // SAFETY: `self.init_set` and all `[MaybeUninit<...>; MAX_*]` arrays are private
                // to this module; `self.init_set.contains(selector)` proves `index` is in bounds
                // and `slot` was initialized in `Allocators::new` and remains live until `Drop`.
                unsafe { slot.assume_init_ref() }
            }
            AllocatorSelector::LargeObject(index) => {
                let slot = &self.large_object[index as usize];
                // SAFETY: Verified initialized and in-bounds via `self.init_set.contains(selector)`.
                unsafe { slot.assume_init_ref() }
            }
            AllocatorSelector::Malloc(index) => {
                let slot = &self.malloc[index as usize];
                // SAFETY: Verified initialized and in-bounds via `self.init_set.contains(selector)`.
                unsafe { slot.assume_init_ref() }
            }
            AllocatorSelector::Immix(index) => {
                let slot = &self.immix[index as usize];
                // SAFETY: Verified initialized and in-bounds via `self.init_set.contains(selector)`.
                unsafe { slot.assume_init_ref() }
            }
            AllocatorSelector::FreeList(index) => {
                let slot = &self.free_list[index as usize];
                // SAFETY: Verified initialized and in-bounds via `self.init_set.contains(selector)`.
                unsafe { slot.assume_init_ref() }
            }
            AllocatorSelector::Lisp2(index) => {
                let slot = &self.lisp2[index as usize];
                // SAFETY: Verified initialized and in-bounds via `self.init_set.contains(selector)`.
                unsafe { slot.assume_init_ref() }
            }
            AllocatorSelector::None => unreachable!(),
        }
    }

    /// Return a typed reference to the initialized allocator selected by `selector`.
    pub(crate) fn get_typed_allocator<T: Allocator<VM>>(&self, selector: AllocatorSelector) -> &T {
        self.get_allocator(selector).downcast_ref().unwrap()
    }

    /// Return a mutable reference to the initialized allocator selected by `selector`.
    /// Panics if `selector` is `AllocatorSelector::None`, out of bounds, or was not initialized.
    pub(crate) fn get_allocator_mut(
        &mut self,
        selector: AllocatorSelector,
    ) -> &mut dyn Allocator<VM> {
        if selector == AllocatorSelector::None {
            panic!("Allocator mapping is not initialized");
        }
        assert!(
            self.init_set.contains(selector),
            "Allocator slot {selector:?} is not initialized"
        );
        match selector {
            AllocatorSelector::BumpPointer(index) => {
                let slot = &mut self.bump_pointer[index as usize];
                // SAFETY: `self.init_set.contains(selector)` proves `index` is in bounds and
                // `slot` was initialized in `Allocators::new`, and `&mut self` grants exclusivity.
                unsafe { slot.assume_init_mut() }
            }
            AllocatorSelector::LargeObject(index) => {
                let slot = &mut self.large_object[index as usize];
                // SAFETY: Verified initialized via `self.init_set` with `&mut self` exclusivity.
                unsafe { slot.assume_init_mut() }
            }
            AllocatorSelector::Malloc(index) => {
                let slot = &mut self.malloc[index as usize];
                // SAFETY: Verified initialized via `self.init_set` with `&mut self` exclusivity.
                unsafe { slot.assume_init_mut() }
            }
            AllocatorSelector::Immix(index) => {
                let slot = &mut self.immix[index as usize];
                // SAFETY: Verified initialized via `self.init_set` with `&mut self` exclusivity.
                unsafe { slot.assume_init_mut() }
            }
            AllocatorSelector::FreeList(index) => {
                let slot = &mut self.free_list[index as usize];
                // SAFETY: Verified initialized via `self.init_set` with `&mut self` exclusivity.
                unsafe { slot.assume_init_mut() }
            }
            AllocatorSelector::Lisp2(index) => {
                let slot = &mut self.lisp2[index as usize];
                // SAFETY: Verified initialized via `self.init_set` with `&mut self` exclusivity.
                unsafe { slot.assume_init_mut() }
            }
            AllocatorSelector::None => unreachable!(),
        }
    }

    /// Return a typed mutable reference to the initialized allocator selected by `selector`.
    pub(crate) fn get_typed_allocator_mut<T: Allocator<VM>>(
        &mut self,
        selector: AllocatorSelector,
    ) -> &mut T {
        self.get_allocator_mut(selector).downcast_mut().unwrap()
    }

    /// Invoke `f` on every initialized allocator slot in `self` without dropping or uninitializing
    /// any slot (leaving `self` valid for repeated calls or subsequent `Drop`).
    pub(crate) fn for_each_initialized_mut(&mut self, mut f: impl FnMut(&mut dyn Allocator<VM>)) {
        for sel in InitSelectorSet::ALL_SELECTORS {
            if self.init_set.contains(sel) {
                f(self.get_allocator_mut(sel));
            }
        }
    }

    pub fn new(
        mutator_tls: VMMutatorThread,
        mmtk: &MMTK<VM>,
        space_mapping: &[(AllocatorSelector, &'static dyn Space<VM>)],
    ) -> Self {
        let mut ret = Allocators {
            bump_pointer: std::array::from_fn(|_| MaybeUninit::uninit()),
            large_object: std::array::from_fn(|_| MaybeUninit::uninit()),
            malloc: std::array::from_fn(|_| MaybeUninit::uninit()),
            immix: std::array::from_fn(|_| MaybeUninit::uninit()),
            free_list: std::array::from_fn(|_| MaybeUninit::uninit()),
            lisp2: std::array::from_fn(|_| MaybeUninit::uninit()),
            init_set: InitSelectorSet::default(),
        };
        let context = Arc::new(AllocatorContext::new(mmtk));

        for &(selector, space) in space_mapping.iter() {
            assert!(
                !ret.init_set.contains(selector),
                "Duplicate selector {selector:?}"
            );
            match selector {
                AllocatorSelector::BumpPointer(index) => {
                    let idx = index as usize;
                    assert!(
                        idx < MAX_BUMP_ALLOCATORS,
                        "BumpPointer selector index out of bounds: {idx}"
                    );
                    ret.bump_pointer[idx].write(BumpAllocator::new(
                        mutator_tls.0,
                        space,
                        context.clone(),
                    ));
                    ret.init_set.insert(selector);
                }
                AllocatorSelector::LargeObject(index) => {
                    let idx = index as usize;
                    assert!(
                        idx < MAX_LARGE_OBJECT_ALLOCATORS,
                        "LargeObject selector index out of bounds: {idx}"
                    );
                    ret.large_object[idx].write(LargeObjectAllocator::new(
                        mutator_tls.0,
                        space.downcast_ref::<LargeObjectSpace<VM>>().unwrap(),
                        context.clone(),
                    ));
                    ret.init_set.insert(selector);
                }
                AllocatorSelector::Malloc(index) => {
                    let idx = index as usize;
                    assert!(
                        idx < MAX_MALLOC_ALLOCATORS,
                        "Malloc selector index out of bounds: {idx}"
                    );
                    ret.malloc[idx].write(MallocAllocator::new(
                        mutator_tls.0,
                        space.downcast_ref::<MallocSpace<VM>>().unwrap(),
                        context.clone(),
                    ));
                    ret.init_set.insert(selector);
                }
                AllocatorSelector::Immix(index) => {
                    let idx = index as usize;
                    assert!(
                        idx < MAX_IMMIX_ALLOCATORS,
                        "Immix selector index out of bounds: {idx}"
                    );
                    ret.immix[idx].write(ImmixAllocator::new(
                        mutator_tls.0,
                        Some(space),
                        context.clone(),
                        false,
                    ));
                    ret.init_set.insert(selector);
                }
                AllocatorSelector::FreeList(index) => {
                    let idx = index as usize;
                    assert!(
                        idx < MAX_FREE_LIST_ALLOCATORS,
                        "FreeList selector index out of bounds: {idx}"
                    );
                    ret.free_list[idx].write(FreeListAllocator::new(
                        mutator_tls.0,
                        space.downcast_ref::<MarkSweepSpace<VM>>().unwrap(),
                        context.clone(),
                    ));
                    ret.init_set.insert(selector);
                }
                AllocatorSelector::Lisp2(index) => {
                    let idx = index as usize;
                    assert!(
                        idx < MAX_LISP2_ALLOCATORS,
                        "Lisp2 selector index out of bounds: {idx}"
                    );
                    ret.lisp2[idx].write(Lisp2Allocator::new(
                        mutator_tls.0,
                        space,
                        context.clone(),
                    ));
                    ret.init_set.insert(selector);
                }
                AllocatorSelector::None => panic!("Allocator mapping is not initialized"),
            }
        }

        ret
    }
}

/// This type describe an allocator in the [`crate::Mutator`].
/// For some VM bindings, they may need to access this type from native code. This type is equivalent to the following native types:
/// #[repr(C)]
/// struct AllocatorSelector {
///   tag: AllocatorSelectorTag,
///   payload: u8,
/// }
/// #[repr(u8)]
/// enum AllocatorSelectorTag {
///   BumpPointer,
///   LargeObject,
///   ...
/// }
#[repr(C, u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum AllocatorSelector {
    /// Represents a [`crate::util::alloc::bumpallocator::BumpAllocator`].
    BumpPointer(u8),
    /// Represents a [`crate::util::alloc::large_object_allocator::LargeObjectAllocator`].
    LargeObject(u8),
    /// Represents a [`crate::util::alloc::malloc_allocator::MallocAllocator`].
    Malloc(u8),
    /// Represents a [`crate::util::alloc::immix_allocator::ImmixAllocator`].
    Immix(u8),
    /// Represents a [`crate::util::alloc::lisp2_allocator::Lisp2Allocator`].
    Lisp2(u8),
    /// Represents a [`crate::util::alloc::free_list_allocator::FreeListAllocator`].
    FreeList(u8),
    /// No allocator found.
    #[default]
    None,
}

/// This type describes allocator information. It is used to
/// generate fast paths for the GC. All offset fields are relative to [`Mutator`].
#[repr(C, u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum AllocatorInfo {
    /// This allocator uses a [`crate::util::alloc::bumpallocator::BumpPointer`] as its fastpath.
    BumpPointer {
        /// The byte offset from the mutator's pointer to the [`crate::util::alloc::bumpallocator::BumpPointer`].
        bump_pointer_offset: usize,
    },
    /// This allocator uses a fastpath, but we haven't implemented it yet.
    // FIXME: Add free-list fast-path
    Unimplemented,
    /// This allocator does not have a fastpath.
    #[default]
    None,
}

impl AllocatorInfo {
    /// Return an AllocatorInfo for the given allocator selector. This method is provided
    /// so that VM compilers may generate allocator fast-path and load fields for the fast-path.
    ///
    /// Arguments:
    /// * `selector`: The allocator selector to query.
    pub fn new<VM: VMBinding>(selector: AllocatorSelector) -> AllocatorInfo {
        let base_offset = Mutator::<VM>::get_allocator_base_offset(selector);
        match selector {
            AllocatorSelector::BumpPointer(_) => {
                let bump_pointer_offset = offset_of!(BumpAllocator<VM>, bump_pointer);

                AllocatorInfo::BumpPointer {
                    bump_pointer_offset: base_offset + bump_pointer_offset,
                }
            }

            AllocatorSelector::Immix(_) => {
                let bump_pointer_offset = offset_of!(ImmixAllocator<VM>, bump_pointer);

                AllocatorInfo::BumpPointer {
                    bump_pointer_offset: base_offset + bump_pointer_offset,
                }
            }

            AllocatorSelector::Lisp2(_) => {
                let bump_offset = base_offset + offset_of!(Lisp2Allocator<VM>, bump_allocator);
                let bump_pointer_offset = offset_of!(BumpAllocator<VM>, bump_pointer);

                AllocatorInfo::BumpPointer {
                    bump_pointer_offset: bump_offset + bump_pointer_offset,
                }
            }

            AllocatorSelector::FreeList(_) => AllocatorInfo::Unimplemented,
            _ => AllocatorInfo::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_selector_set_maps_all_valid_slots_uniquely() {
        let valid_selectors = [
            AllocatorSelector::BumpPointer(0),
            AllocatorSelector::BumpPointer(1),
            AllocatorSelector::BumpPointer(2),
            AllocatorSelector::BumpPointer(3),
            AllocatorSelector::BumpPointer(4),
            AllocatorSelector::BumpPointer(5),
            AllocatorSelector::LargeObject(0),
            AllocatorSelector::LargeObject(1),
            AllocatorSelector::Malloc(0),
            AllocatorSelector::Immix(0),
            AllocatorSelector::Immix(1),
            AllocatorSelector::FreeList(0),
            AllocatorSelector::FreeList(1),
            AllocatorSelector::Lisp2(0),
        ];

        let mut set = InitSelectorSet::default();
        let mut seen_bits = 0u16;
        for &sel in &valid_selectors {
            assert!(!set.contains(sel));
            let bit = InitSelectorSet::bit_index(sel).expect("expected valid bit index");
            assert!(bit < 14);
            assert_eq!(seen_bits & (1u16 << bit), 0, "duplicate bit for {sel:?}");
            seen_bits |= 1u16 << bit;
            set.insert(sel);
            assert!(set.contains(sel));
        }
        assert_eq!(seen_bits, (1u16 << 14) - 1);

        assert!(!set.contains(AllocatorSelector::None));
        assert!(!set.contains(AllocatorSelector::BumpPointer(6)));
        assert!(!set.contains(AllocatorSelector::LargeObject(2)));
        assert!(!set.contains(AllocatorSelector::Malloc(1)));
        assert!(!set.contains(AllocatorSelector::Immix(2)));
        assert!(!set.contains(AllocatorSelector::FreeList(2)));
        assert!(!set.contains(AllocatorSelector::Lisp2(1)));
    }

    #[test]
    #[should_panic(expected = "Out-of-bounds or invalid AllocatorSelector")]
    fn init_selector_set_rejects_out_of_bounds_insert() {
        let mut set = InitSelectorSet::default();
        set.insert(AllocatorSelector::BumpPointer(MAX_BUMP_ALLOCATORS as u8));
    }

    #[cfg(feature = "mock_test")]
    #[test]
    fn repr_c_allocators_layout_matches_migration_guide() {
        use crate::util::test_util::mock_vm::MockVM;
        use std::mem::align_of;

        assert_eq!(size_of::<InitSelectorSet>(), size_of::<u16>());
        assert_eq!(align_of::<Allocators<MockVM>>(), size_of::<usize>());
        assert_eq!(
            offset_of!(Allocators<MockVM>, init_set) + size_of::<usize>(),
            size_of::<Allocators<MockVM>>()
        );
        assert_eq!(
            offset_of!(Mutator<MockVM>, barrier),
            size_of::<Allocators<MockVM>>()
        );
    }
}
