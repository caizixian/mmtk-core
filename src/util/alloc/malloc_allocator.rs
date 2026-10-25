#![deny(unsafe_op_in_unsafe_fn)]

use std::sync::Arc;

use crate::policy::marksweepspace::malloc_ms::MallocSpace;
use crate::policy::space::Space;
use crate::util::alloc::Allocator;
use crate::util::opaque_pointer::*;
use crate::util::Address;
use crate::vm::VMBinding;

use super::allocator::AllocatorContext;

/// The allocator that internally uses malloc for all the allocation requests.
/// This allocator is only intended for experimental uses.
#[repr(C)]
pub struct MallocAllocator<VM: VMBinding> {
    /// [`VMThread`] associated with this allocator instance
    pub tls: VMThread,
    /// [`Space`](src/policy/space/Space) instance associated with this allocator instance.
    space: Option<&'static MallocSpace<VM>>,
    context: Arc<AllocatorContext<VM>>,
}

impl<VM: VMBinding> Allocator<VM> for MallocAllocator<VM> {
    fn get_space(&self) -> &'static dyn Space<VM> {
        self.malloc_space() as &'static dyn Space<VM>
    }

    fn get_context(&self) -> &AllocatorContext<VM> {
        &self.context
    }

    fn alloc(&mut self, size: usize, align: usize, offset: usize) -> Address {
        self.alloc_slow(size, align, offset)
    }

    fn get_tls(&self) -> VMThread {
        self.tls
    }

    fn does_thread_local_allocation(&self) -> bool {
        false
    }

    fn alloc_slow_once(&mut self, size: usize, align: usize, offset: usize) -> Address {
        self.malloc_space().alloc(self.tls, size, align, offset)
    }
}

impl<VM: VMBinding> MallocAllocator<VM> {
    pub(in crate::util::alloc) fn new_uninit(
        tls: VMThread,
        context: Arc<AllocatorContext<VM>>,
    ) -> Self {
        MallocAllocator {
            tls,
            space: None,
            context,
        }
    }

    #[inline(always)]
    pub(in crate::util::alloc) fn is_initialized(&self) -> bool {
        self.space.is_some()
    }

    #[inline(always)]
    fn malloc_space(&self) -> &'static MallocSpace<VM> {
        self.space.expect("MallocAllocator slot is not initialized")
    }

    pub(crate) fn new(
        tls: VMThread,
        space: &'static MallocSpace<VM>,
        context: Arc<AllocatorContext<VM>>,
    ) -> Self {
        MallocAllocator {
            tls,
            space: Some(space),
            context,
        }
    }
}
