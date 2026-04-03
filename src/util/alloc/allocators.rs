use std::mem::MaybeUninit;
use std::sync::Arc;

use memoffset::offset_of;

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
use super::MarkCompactAllocator;

pub(crate) const MAX_BUMP_ALLOCATORS: usize = 6;
pub(crate) const MAX_LARGE_OBJECT_ALLOCATORS: usize = 2;
pub(crate) const MAX_MALLOC_ALLOCATORS: usize = 1;
pub(crate) const MAX_IMMIX_ALLOCATORS: usize = 2;
pub(crate) const MAX_FREE_LIST_ALLOCATORS: usize = 2;
pub(crate) const MAX_MARK_COMPACT_ALLOCATORS: usize = 1;

// The allocators set owned by each mutator. We provide a fixed number of allocators for each allocator type in the mutator,
// and each plan will select part of the allocators to use.
// Note that this struct is part of the Mutator struct.
// We are trying to make it fixed-sized so that VM bindings can easily define a Mutator type to have the exact same layout as our Mutator struct.
#[repr(C)]
pub struct Allocators<VM: VMBinding> {
    pub bump_pointer: [MaybeUninit<BumpAllocator<VM>>; MAX_BUMP_ALLOCATORS],
    pub large_object: [MaybeUninit<LargeObjectAllocator<VM>>; MAX_LARGE_OBJECT_ALLOCATORS],
    pub malloc: [MaybeUninit<MallocAllocator<VM>>; MAX_MALLOC_ALLOCATORS],
    pub immix: [MaybeUninit<ImmixAllocator<VM>>; MAX_IMMIX_ALLOCATORS],
    pub free_list: [MaybeUninit<FreeListAllocator<VM>>; MAX_FREE_LIST_ALLOCATORS],
    pub markcompact: [MaybeUninit<MarkCompactAllocator<VM>>; MAX_MARK_COMPACT_ALLOCATORS],
    pub initialized_bitmap: u32,
}

pub trait HasAllocatorArray<VM: VMBinding>: Sized {
    fn get_array(allocators: &Allocators<VM>) -> &[MaybeUninit<Self>];
    fn get_array_mut(allocators: &mut Allocators<VM>) -> &mut [MaybeUninit<Self>];
    fn get_index(selector: AllocatorSelector) -> Option<usize>;
    fn get_bit_offset(index: usize) -> usize;
}

impl<VM: VMBinding> HasAllocatorArray<VM> for BumpAllocator<VM> {
    fn get_array(allocators: &Allocators<VM>) -> &[MaybeUninit<Self>] { &allocators.bump_pointer }
    fn get_array_mut(allocators: &mut Allocators<VM>) -> &mut [MaybeUninit<Self>] { &mut allocators.bump_pointer }
    fn get_index(selector: AllocatorSelector) -> Option<usize> {
        if let AllocatorSelector::BumpPointer(i) = selector { Some(i as usize) } else { None }
    }
    fn get_bit_offset(index: usize) -> usize { index }
}

impl<VM: VMBinding> HasAllocatorArray<VM> for LargeObjectAllocator<VM> {
    fn get_array(allocators: &Allocators<VM>) -> &[MaybeUninit<Self>] { &allocators.large_object }
    fn get_array_mut(allocators: &mut Allocators<VM>) -> &mut [MaybeUninit<Self>] { &mut allocators.large_object }
    fn get_index(selector: AllocatorSelector) -> Option<usize> {
        if let AllocatorSelector::LargeObject(i) = selector { Some(i as usize) } else { None }
    }
    fn get_bit_offset(index: usize) -> usize { MAX_BUMP_ALLOCATORS + index }
}

impl<VM: VMBinding> HasAllocatorArray<VM> for MallocAllocator<VM> {
    fn get_array(allocators: &Allocators<VM>) -> &[MaybeUninit<Self>] { &allocators.malloc }
    fn get_array_mut(allocators: &mut Allocators<VM>) -> &mut [MaybeUninit<Self>] { &mut allocators.malloc }
    fn get_index(selector: AllocatorSelector) -> Option<usize> {
        if let AllocatorSelector::Malloc(i) = selector { Some(i as usize) } else { None }
    }
    fn get_bit_offset(index: usize) -> usize { MAX_BUMP_ALLOCATORS + MAX_LARGE_OBJECT_ALLOCATORS + index }
}

impl<VM: VMBinding> HasAllocatorArray<VM> for ImmixAllocator<VM> {
    fn get_array(allocators: &Allocators<VM>) -> &[MaybeUninit<Self>] { &allocators.immix }
    fn get_array_mut(allocators: &mut Allocators<VM>) -> &mut [MaybeUninit<Self>] { &mut allocators.immix }
    fn get_index(selector: AllocatorSelector) -> Option<usize> {
        if let AllocatorSelector::Immix(i) = selector { Some(i as usize) } else { None }
    }
    fn get_bit_offset(index: usize) -> usize { MAX_BUMP_ALLOCATORS + MAX_LARGE_OBJECT_ALLOCATORS + MAX_MALLOC_ALLOCATORS + index }
}

impl<VM: VMBinding> HasAllocatorArray<VM> for FreeListAllocator<VM> {
    fn get_array(allocators: &Allocators<VM>) -> &[MaybeUninit<Self>] { &allocators.free_list }
    fn get_array_mut(allocators: &mut Allocators<VM>) -> &mut [MaybeUninit<Self>] { &mut allocators.free_list }
    fn get_index(selector: AllocatorSelector) -> Option<usize> {
        if let AllocatorSelector::FreeList(i) = selector { Some(i as usize) } else { None }
    }
    fn get_bit_offset(index: usize) -> usize { MAX_BUMP_ALLOCATORS + MAX_LARGE_OBJECT_ALLOCATORS + MAX_MALLOC_ALLOCATORS + MAX_IMMIX_ALLOCATORS + index }
}

impl<VM: VMBinding> HasAllocatorArray<VM> for MarkCompactAllocator<VM> {
    fn get_array(allocators: &Allocators<VM>) -> &[MaybeUninit<Self>] { &allocators.markcompact }
    fn get_array_mut(allocators: &mut Allocators<VM>) -> &mut [MaybeUninit<Self>] { &mut allocators.markcompact }
    fn get_index(selector: AllocatorSelector) -> Option<usize> {
        if let AllocatorSelector::MarkCompact(i) = selector { Some(i as usize) } else { None }
    }
    fn get_bit_offset(index: usize) -> usize { MAX_BUMP_ALLOCATORS + MAX_LARGE_OBJECT_ALLOCATORS + MAX_MALLOC_ALLOCATORS + MAX_IMMIX_ALLOCATORS + MAX_FREE_LIST_ALLOCATORS + index }
}

impl<VM: VMBinding> Allocators<VM> {
    pub fn is_initialized<T: HasAllocatorArray<VM>>(&self, index: usize) -> bool {
        let bit = T::get_bit_offset(index);
        (self.initialized_bitmap & (1 << bit)) != 0
    }

    pub fn set_initialized<T: HasAllocatorArray<VM>>(&mut self, index: usize) {
        let bit = T::get_bit_offset(index);
        self.initialized_bitmap |= 1 << bit;
    }

    /// # Safety
    /// The selector needs to be valid, and points to an allocator that has been initialized.
    pub fn get_allocator(&self, selector: AllocatorSelector) -> &dyn Allocator<VM> {
        let bit = selector.get_bit_offset();
        assert!((self.initialized_bitmap & (1 << bit)) != 0, "Allocator not initialized");
        // SAFETY: we checked that it is initialized
        unsafe {
            match selector {
                AllocatorSelector::BumpPointer(index) => self.bump_pointer[index as usize].assume_init_ref(),
                AllocatorSelector::LargeObject(index) => self.large_object[index as usize].assume_init_ref(),
                AllocatorSelector::Malloc(index) => self.malloc[index as usize].assume_init_ref(),
                AllocatorSelector::Immix(index) => self.immix[index as usize].assume_init_ref(),
                AllocatorSelector::FreeList(index) => self.free_list[index as usize].assume_init_ref(),
                AllocatorSelector::MarkCompact(index) => self.markcompact[index as usize].assume_init_ref(),
                AllocatorSelector::None => panic!("Allocator mapping is not initialized"),
            }
        }
    }

    /// # Safety
    /// The selector needs to be valid, and points to an allocator that has been initialized.
    pub fn get_typed_allocator<T: HasAllocatorArray<VM>>(&self, selector: AllocatorSelector) -> &T {
        let index = T::get_index(selector).expect("Selector does not match allocator type");
        assert!(self.is_initialized::<T>(index), "Allocator not initialized");
        unsafe { T::get_array(self)[index].assume_init_ref() }
    }

    /// # Safety
    /// The selector needs to be valid, and points to an allocator that has been initialized.
    pub fn get_allocator_mut(
        &mut self,
        selector: AllocatorSelector,
    ) -> &mut dyn Allocator<VM> {
        let bit = selector.get_bit_offset();
        assert!((self.initialized_bitmap & (1 << bit)) != 0, "Allocator not initialized");
        // SAFETY: we checked that it is initialized
        unsafe {
            match selector {
                AllocatorSelector::BumpPointer(index) => self.bump_pointer[index as usize].assume_init_mut(),
                AllocatorSelector::LargeObject(index) => self.large_object[index as usize].assume_init_mut(),
                AllocatorSelector::Malloc(index) => self.malloc[index as usize].assume_init_mut(),
                AllocatorSelector::Immix(index) => self.immix[index as usize].assume_init_mut(),
                AllocatorSelector::FreeList(index) => self.free_list[index as usize].assume_init_mut(),
                AllocatorSelector::MarkCompact(index) => self.markcompact[index as usize].assume_init_mut(),
                AllocatorSelector::None => panic!("Allocator mapping is not initialized"),
            }
        }
    }

    /// # Safety
    /// The selector needs to be valid, and points to an allocator that has been initialized.
    pub fn get_typed_allocator_mut<T: HasAllocatorArray<VM>>(
        &mut self,
        selector: AllocatorSelector,
    ) -> &mut T {
        let index = T::get_index(selector).expect("Selector does not match allocator type");
        assert!(self.is_initialized::<T>(index), "Allocator not initialized");
        unsafe { T::get_array_mut(self)[index].assume_init_mut() }
    }

    pub fn new(
        mutator_tls: VMMutatorThread,
        mmtk: &MMTK<VM>,
        space_mapping: &[(AllocatorSelector, &'static dyn Space<VM>)],
    ) -> Self {
        let mut ret = Allocators {
            bump_pointer: [const { MaybeUninit::uninit() }; MAX_BUMP_ALLOCATORS],
            large_object: [const { MaybeUninit::uninit() }; MAX_LARGE_OBJECT_ALLOCATORS],
            malloc: [const { MaybeUninit::uninit() }; MAX_MALLOC_ALLOCATORS],
            immix: [const { MaybeUninit::uninit() }; MAX_IMMIX_ALLOCATORS],
            free_list: [const { MaybeUninit::uninit() }; MAX_FREE_LIST_ALLOCATORS],
            markcompact: [const { MaybeUninit::uninit() }; MAX_MARK_COMPACT_ALLOCATORS],
            initialized_bitmap: 0,
        };
        let context = Arc::new(AllocatorContext::new(mmtk));

        for &(selector, space) in space_mapping.iter() {
            match selector {
                AllocatorSelector::BumpPointer(index) => {
                    ret.bump_pointer[index as usize].write(BumpAllocator::new(
                        mutator_tls.0,
                        space,
                        context.clone(),
                    ));
                    ret.set_initialized::<BumpAllocator<VM>>(index as usize);
                }
                AllocatorSelector::LargeObject(index) => {
                    ret.large_object[index as usize].write(LargeObjectAllocator::new(
                        mutator_tls.0,
                        space.downcast_ref::<LargeObjectSpace<VM>>().unwrap(),
                        context.clone(),
                    ));
                    ret.set_initialized::<LargeObjectAllocator<VM>>(index as usize);
                }
                AllocatorSelector::Malloc(index) => {
                    ret.malloc[index as usize].write(MallocAllocator::new(
                        mutator_tls.0,
                        space.downcast_ref::<MallocSpace<VM>>().unwrap(),
                        context.clone(),
                    ));
                    ret.set_initialized::<MallocAllocator<VM>>(index as usize);
                }
                AllocatorSelector::Immix(index) => {
                    ret.immix[index as usize].write(ImmixAllocator::new(
                        mutator_tls.0,
                        Some(space),
                        context.clone(),
                        false,
                    ));
                    ret.set_initialized::<ImmixAllocator<VM>>(index as usize);
                }
                AllocatorSelector::FreeList(index) => {
                    ret.free_list[index as usize].write(FreeListAllocator::new(
                        mutator_tls.0,
                        space.downcast_ref::<MarkSweepSpace<VM>>().unwrap(),
                        context.clone(),
                    ));
                    ret.set_initialized::<FreeListAllocator<VM>>(index as usize);
                }
                AllocatorSelector::MarkCompact(index) => {
                    ret.markcompact[index as usize].write(MarkCompactAllocator::new(
                        mutator_tls.0,
                        space,
                        context.clone(),
                    ));
                    ret.set_initialized::<MarkCompactAllocator<VM>>(index as usize);
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
    BumpPointer(u8),
    LargeObject(u8),
    Malloc(u8),
    Immix(u8),
    MarkCompact(u8),
    FreeList(u8),
    #[default]
    None,
}

impl AllocatorSelector {
    pub fn get_bit_offset(self) -> usize {
        match self {
            AllocatorSelector::BumpPointer(i) => i as usize,
            AllocatorSelector::LargeObject(i) => MAX_BUMP_ALLOCATORS + i as usize,
            AllocatorSelector::Malloc(i) => MAX_BUMP_ALLOCATORS + MAX_LARGE_OBJECT_ALLOCATORS + i as usize,
            AllocatorSelector::Immix(i) => MAX_BUMP_ALLOCATORS + MAX_LARGE_OBJECT_ALLOCATORS + MAX_MALLOC_ALLOCATORS + i as usize,
            AllocatorSelector::FreeList(i) => MAX_BUMP_ALLOCATORS + MAX_LARGE_OBJECT_ALLOCATORS + MAX_MALLOC_ALLOCATORS + MAX_IMMIX_ALLOCATORS + i as usize,
            AllocatorSelector::MarkCompact(i) => MAX_BUMP_ALLOCATORS + MAX_LARGE_OBJECT_ALLOCATORS + MAX_MALLOC_ALLOCATORS + MAX_IMMIX_ALLOCATORS + MAX_FREE_LIST_ALLOCATORS + i as usize,
            AllocatorSelector::None => panic!("Cannot get bit offset for None"),
        }
    }

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

            AllocatorSelector::MarkCompact(_) => {
                let bump_offset =
                    base_offset + offset_of!(MarkCompactAllocator<VM>, bump_allocator);
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
