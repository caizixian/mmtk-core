use std::mem::{offset_of, size_of};
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

/// The allocator set owned by each [`Mutator`].
///
/// Every slot in all 6 `#[repr(C)]` arrays is safely initialized at construction: unconfigured
/// slots store `space: None` using Rust's guaranteed null-pointer niche optimization on
/// `Option<&'static ...>`, preserving exact inline sizes and fast-path `(tls, bump_pointer)`
/// offsets without `MaybeUninit`, bitmasks, or manual `Drop`.
#[repr(C)]
pub struct Allocators<VM: VMBinding> {
    bump_pointer: [BumpAllocator<VM>; MAX_BUMP_ALLOCATORS],
    large_object: [LargeObjectAllocator<VM>; MAX_LARGE_OBJECT_ALLOCATORS],
    malloc: [MallocAllocator<VM>; MAX_MALLOC_ALLOCATORS],
    immix: [ImmixAllocator<VM>; MAX_IMMIX_ALLOCATORS],
    free_list: [FreeListAllocator<VM>; MAX_FREE_LIST_ALLOCATORS],
    lisp2: [Lisp2Allocator<VM>; MAX_LISP2_ALLOCATORS],
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
        match selector {
            AllocatorSelector::BumpPointer(index) => {
                let slot = &self.bump_pointer[index as usize];
                assert!(
                    slot.is_initialized(),
                    "Allocator slot {selector:?} is not initialized"
                );
                slot
            }
            AllocatorSelector::LargeObject(index) => {
                let slot = &self.large_object[index as usize];
                assert!(
                    slot.is_initialized(),
                    "Allocator slot {selector:?} is not initialized"
                );
                slot
            }
            AllocatorSelector::Malloc(index) => {
                let slot = &self.malloc[index as usize];
                assert!(
                    slot.is_initialized(),
                    "Allocator slot {selector:?} is not initialized"
                );
                slot
            }
            AllocatorSelector::Immix(index) => {
                let slot = &self.immix[index as usize];
                assert!(
                    slot.is_initialized(),
                    "Allocator slot {selector:?} is not initialized"
                );
                slot
            }
            AllocatorSelector::FreeList(index) => {
                let slot = &self.free_list[index as usize];
                assert!(
                    slot.is_initialized(),
                    "Allocator slot {selector:?} is not initialized"
                );
                slot
            }
            AllocatorSelector::Lisp2(index) => {
                let slot = &self.lisp2[index as usize];
                assert!(
                    slot.is_initialized(),
                    "Allocator slot {selector:?} is not initialized"
                );
                slot
            }
            AllocatorSelector::None => panic!("Allocator mapping is not initialized"),
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
        match selector {
            AllocatorSelector::BumpPointer(index) => {
                let slot = &mut self.bump_pointer[index as usize];
                assert!(
                    slot.is_initialized(),
                    "Allocator slot {selector:?} is not initialized"
                );
                slot
            }
            AllocatorSelector::LargeObject(index) => {
                let slot = &mut self.large_object[index as usize];
                assert!(
                    slot.is_initialized(),
                    "Allocator slot {selector:?} is not initialized"
                );
                slot
            }
            AllocatorSelector::Malloc(index) => {
                let slot = &mut self.malloc[index as usize];
                assert!(
                    slot.is_initialized(),
                    "Allocator slot {selector:?} is not initialized"
                );
                slot
            }
            AllocatorSelector::Immix(index) => {
                let slot = &mut self.immix[index as usize];
                assert!(
                    slot.is_initialized(),
                    "Allocator slot {selector:?} is not initialized"
                );
                slot
            }
            AllocatorSelector::FreeList(index) => {
                let slot = &mut self.free_list[index as usize];
                assert!(
                    slot.is_initialized(),
                    "Allocator slot {selector:?} is not initialized"
                );
                slot
            }
            AllocatorSelector::Lisp2(index) => {
                let slot = &mut self.lisp2[index as usize];
                assert!(
                    slot.is_initialized(),
                    "Allocator slot {selector:?} is not initialized"
                );
                slot
            }
            AllocatorSelector::None => panic!("Allocator mapping is not initialized"),
        }
    }

    /// Return a typed mutable reference to the initialized allocator selected by `selector`.
    pub(crate) fn get_typed_allocator_mut<T: Allocator<VM>>(
        &mut self,
        selector: AllocatorSelector,
    ) -> &mut T {
        self.get_allocator_mut(selector).downcast_mut().unwrap()
    }

    /// Invoke `f` on every configured allocator slot in `self`.
    pub(crate) fn for_each_initialized_mut(&mut self, mut f: impl FnMut(&mut dyn Allocator<VM>)) {
        for slot in &mut self.bump_pointer {
            if slot.is_initialized() {
                f(slot);
            }
        }
        for slot in &mut self.large_object {
            if slot.is_initialized() {
                f(slot);
            }
        }
        for slot in &mut self.malloc {
            if slot.is_initialized() {
                f(slot);
            }
        }
        for slot in &mut self.immix {
            if slot.is_initialized() {
                f(slot);
            }
        }
        for slot in &mut self.free_list {
            if slot.is_initialized() {
                f(slot);
            }
        }
        for slot in &mut self.lisp2 {
            if slot.is_initialized() {
                f(slot);
            }
        }
    }

    pub fn new(
        mutator_tls: VMMutatorThread,
        mmtk: &MMTK<VM>,
        space_mapping: &[(AllocatorSelector, &'static dyn Space<VM>)],
    ) -> Self {
        let context = Arc::new(AllocatorContext::new(mmtk));
        let tls = mutator_tls.0;
        let mut ret = Allocators {
            bump_pointer: std::array::from_fn(|_| BumpAllocator::new_uninit(tls, context.clone())),
            large_object: std::array::from_fn(|_| {
                LargeObjectAllocator::new_uninit(tls, context.clone())
            }),
            malloc: std::array::from_fn(|_| MallocAllocator::new_uninit(tls, context.clone())),
            immix: std::array::from_fn(|_| ImmixAllocator::new_uninit(tls, context.clone())),
            free_list: std::array::from_fn(|_| FreeListAllocator::new_uninit(tls, context.clone())),
            lisp2: std::array::from_fn(|_| Lisp2Allocator::new_uninit(tls, context.clone())),
        };

        for &(selector, space) in space_mapping.iter() {
            match selector {
                AllocatorSelector::BumpPointer(index) => {
                    let slot = &mut ret.bump_pointer[index as usize];
                    assert!(!slot.is_initialized(), "Duplicate selector {selector:?}");
                    *slot = BumpAllocator::new(tls, space, context.clone());
                }
                AllocatorSelector::LargeObject(index) => {
                    let slot = &mut ret.large_object[index as usize];
                    assert!(!slot.is_initialized(), "Duplicate selector {selector:?}");
                    *slot = LargeObjectAllocator::new(
                        tls,
                        space.downcast_ref::<LargeObjectSpace<VM>>().unwrap(),
                        context.clone(),
                    );
                }
                AllocatorSelector::Malloc(index) => {
                    let slot = &mut ret.malloc[index as usize];
                    assert!(!slot.is_initialized(), "Duplicate selector {selector:?}");
                    *slot = MallocAllocator::new(
                        tls,
                        space.downcast_ref::<MallocSpace<VM>>().unwrap(),
                        context.clone(),
                    );
                }
                AllocatorSelector::Immix(index) => {
                    let slot = &mut ret.immix[index as usize];
                    assert!(!slot.is_initialized(), "Duplicate selector {selector:?}");
                    *slot = ImmixAllocator::new(tls, Some(space), context.clone(), false);
                }
                AllocatorSelector::FreeList(index) => {
                    let slot = &mut ret.free_list[index as usize];
                    assert!(!slot.is_initialized(), "Duplicate selector {selector:?}");
                    *slot = FreeListAllocator::new(
                        tls,
                        space.downcast_ref::<MarkSweepSpace<VM>>().unwrap(),
                        context.clone(),
                    );
                }
                AllocatorSelector::Lisp2(index) => {
                    let slot = &mut ret.lisp2[index as usize];
                    assert!(!slot.is_initialized(), "Duplicate selector {selector:?}");
                    *slot = Lisp2Allocator::new(tls, space, context.clone());
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

#[cfg(all(test, feature = "mock_test"))]
mod tests {
    use super::*;
    use crate::util::test_util::mock_vm::MockVM;
    use std::mem::align_of;

    #[test]
    fn repr_c_allocators_layout_matches_baseline_mutator() {
        let word = size_of::<usize>();
        assert_eq!(align_of::<Allocators<MockVM>>(), word);
        assert_eq!(offset_of!(BumpAllocator<MockVM>, bump_pointer), word);
        assert_eq!(offset_of!(ImmixAllocator<MockVM>, bump_pointer), word);
        assert_eq!(size_of::<BumpAllocator<MockVM>>(), 6 * word);
        assert_eq!(size_of::<LargeObjectAllocator<MockVM>>(), 3 * word);
        assert_eq!(size_of::<MallocAllocator<MockVM>>(), 3 * word);
        assert_eq!(size_of::<FreeListAllocator<MockVM>>(), 7 * word);
        assert_eq!(
            offset_of!(Allocators<MockVM>, lisp2) + size_of::<Lisp2Allocator<MockVM>>(),
            size_of::<Allocators<MockVM>>()
        );
        assert_eq!(
            offset_of!(Mutator<MockVM>, barrier),
            size_of::<Allocators<MockVM>>()
        );
    }
}
