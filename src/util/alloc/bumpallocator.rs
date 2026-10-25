use std::sync::Arc;

use crate::util::Address;

use crate::util::alloc::Allocator;

use crate::policy::space::Space;
use crate::util::conversions::bytes_to_pages_up;
use crate::util::opaque_pointer::*;
use crate::vm::VMBinding;

/// Size of a bump allocator block. Currently it is set to 32 KB.
const BLOCK_SIZE: usize = 8 << crate::util::constants::LOG_BYTES_IN_PAGE;
const BLOCK_MASK: usize = BLOCK_SIZE - 1;

/// A bump pointer allocator. It keeps a thread local allocation buffer,
/// and bumps a cursor to allocate from the buffer.
#[repr(C)]
pub struct BumpAllocator<VM: VMBinding> {
    /// [`VMThread`] associated with this allocator instance
    pub tls: VMThread,
    /// Bump-pointer itself.
    pub bump_pointer: BumpPointer,
    /// [`Space`](src/policy/space/Space) instance associated with this allocator instance,
    /// or `None` for unconfigured slots in [`super::allocators::Allocators`].
    space: Option<&'static dyn Space<VM>>,
    pub(in crate::util::alloc) context: Arc<AllocatorContext<VM>>,
}

/// A common fast-path bump-pointer allocator shared across different allocator implementations
/// that use bump-pointer allocation.
/// A `BumpPointer` is always initialized with cursor = 0, limit = 0, so the first allocation
/// always fails the check of `cursor + size < limit` and goes to the slowpath. A binding
/// can also take advantage of this design to zero-initialize the a bump pointer.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct BumpPointer {
    /// The cursor inside the allocation buffer where the next object will be allocated.
    pub cursor: Address,
    /// The upperbound of the allocation buffer.
    pub limit: Address,
}

impl BumpPointer {
    /// Reset the cursor and limit to the given values.
    pub fn reset(&mut self, start: Address, end: Address) {
        self.cursor = start;
        self.limit = end;
    }

    /// Set `cursor = start` and encode `remaining_bytes` into `limit` (`< cursor`) so fast-path
    /// `cursor + size > limit` trips into `alloc_slow_once_precise_stress`.
    #[inline]
    pub(crate) fn reset_for_stress_test(&mut self, start: Address, remaining_bytes: usize) {
        assert!(
            remaining_bytes == 0 || start.as_usize() > remaining_bytes,
            "Stress-test bump buffer start ({}) must exceed remaining_bytes ({})",
            start,
            remaining_bytes
        );
        self.cursor = start;
        // SAFETY: `Address::from_usize` wraps `remaining_bytes` as an integer sentinel (`< cursor`
        // when non-zero) without Rust-level UB; this sentinel is never dereferenced as a pointer
        // and is restored via `cursor + limit.as_usize()`.
        self.limit = unsafe { Address::from_usize(remaining_bytes) };
    }

    /// Fold an active `[cursor, limit)` buffer into a stress-test sentinel limit (`limit - cursor`).
    /// No-op if `limit <= cursor` (already folded or empty).
    #[inline]
    pub(crate) fn fold_limit_for_stress_test(&mut self) -> Option<(Address, Address)> {
        if self.cursor < self.limit {
            let old_limit = self.limit;
            let remaining = self.limit - self.cursor;
            self.reset_for_stress_test(self.cursor, remaining);
            Some((old_limit, self.limit))
        } else {
            None
        }
    }

    /// Restore a stress-folded `[cursor, limit)` buffer back to `cursor + limit.as_usize()`.
    /// No-op if `limit >= cursor` (already restored or empty).
    #[inline]
    pub(crate) fn restore_limit_for_stress_test(&mut self) -> Option<(Address, Address)> {
        if self.limit < self.cursor {
            let old_limit = self.limit;
            self.limit = self.cursor + self.limit.as_usize();
            Some((old_limit, self.limit))
        } else {
            None
        }
    }
}

impl std::default::Default for BumpPointer {
    /// Defaults to 0,0. In this case, the first
    /// allocation would naturally fail the check
    /// `cursor + size < limit`, and go to the slowpath.
    fn default() -> Self {
        BumpPointer {
            cursor: Address::ZERO,
            limit: Address::ZERO,
        }
    }
}

impl<VM: VMBinding> BumpAllocator<VM> {
    pub(crate) fn set_limit(&mut self, start: Address, limit: Address) {
        self.bump_pointer.reset(start, limit);
    }

    pub(crate) fn reset(&mut self) {
        self.bump_pointer.reset(Address::ZERO, Address::ZERO);
    }

    pub(crate) fn rebind(&mut self, space: &'static dyn Space<VM>) {
        self.reset();
        self.space = Some(space);
    }
}

use crate::util::alloc::allocator::align_allocation_no_fill;
use crate::util::alloc::fill_alignment_gap;

use super::allocator::AllocatorContext;

impl<VM: VMBinding> Allocator<VM> for BumpAllocator<VM> {
    fn get_space(&self) -> &'static dyn Space<VM> {
        self.space.expect("BumpAllocator slot is not initialized")
    }

    fn get_context(&self) -> &AllocatorContext<VM> {
        &self.context
    }

    fn does_thread_local_allocation(&self) -> bool {
        true
    }

    fn get_thread_local_buffer_granularity(&self) -> usize {
        BLOCK_SIZE
    }

    fn alloc(&mut self, size: usize, align: usize, offset: usize) -> Address {
        trace!("alloc");
        let result = align_allocation_no_fill::<VM>(self.bump_pointer.cursor, align, offset);
        let new_cursor = result + size;

        if new_cursor < result || new_cursor > self.bump_pointer.limit {
            trace!("Thread local buffer used up, go to alloc slow path");
            self.alloc_slow(size, align, offset)
        } else {
            fill_alignment_gap::<VM>(self.bump_pointer.cursor, result);
            self.bump_pointer.cursor = new_cursor;
            trace!(
                "Bump allocation size: {}, result: {}, new_cursor: {}, limit: {}",
                size,
                result,
                self.bump_pointer.cursor,
                self.bump_pointer.limit
            );
            result
        }
    }

    fn alloc_slow_once(&mut self, size: usize, align: usize, offset: usize) -> Address {
        trace!("alloc_slow");
        self.acquire_block(size, align, offset, false)
    }

    /// Slow path for allocation if precise stress testing has been enabled.
    /// It works by manipulating the limit to be always below the cursor.
    /// Can have three different cases:
    ///  - acquires a new block if the hard limit has been met;
    ///  - allocates an object using the bump pointer semantics from the
    ///    fastpath if there is sufficient space; and
    ///  - does not allocate an object but forces a poll for GC if the stress
    ///    factor has been crossed.
    fn alloc_slow_once_precise_stress(
        &mut self,
        size: usize,
        align: usize,
        offset: usize,
        need_poll: bool,
    ) -> Address {
        if need_poll {
            return self.acquire_block(size, align, offset, true);
        }

        trace!("alloc_slow stress_test");
        let result = align_allocation_no_fill::<VM>(self.bump_pointer.cursor, align, offset);
        let new_cursor = result + size;

        // For stress test, limit is [0, block_size) to artificially make the
        // check in the fastpath (alloc()) fail. The real limit is recovered by
        // adding it to the current cursor.
        if new_cursor < result
            || new_cursor > self.bump_pointer.cursor + self.bump_pointer.limit.as_usize()
        {
            self.acquire_block(size, align, offset, true)
        } else {
            fill_alignment_gap::<VM>(self.bump_pointer.cursor, result);
            self.bump_pointer.limit -= new_cursor - self.bump_pointer.cursor;
            self.bump_pointer.cursor = new_cursor;
            trace!(
                "alloc_slow: Bump allocation size: {}, result: {}, new_cursor: {}, limit: {}",
                size,
                result,
                self.bump_pointer.cursor,
                self.bump_pointer.limit
            );
            result
        }
    }

    fn get_tls(&self) -> VMThread {
        self.tls
    }
}

impl<VM: VMBinding> BumpAllocator<VM> {
    pub(in crate::util::alloc) fn new_uninit(
        tls: VMThread,
        context: Arc<AllocatorContext<VM>>,
    ) -> Self {
        BumpAllocator {
            tls,
            bump_pointer: BumpPointer::default(),
            space: None,
            context,
        }
    }

    #[inline(always)]
    pub(in crate::util::alloc) fn is_initialized(&self) -> bool {
        self.space.is_some()
    }

    pub(crate) fn new(
        tls: VMThread,
        space: &'static dyn Space<VM>,
        context: Arc<AllocatorContext<VM>>,
    ) -> Self {
        BumpAllocator {
            tls,
            bump_pointer: BumpPointer::default(),
            space: Some(space),
            context,
        }
    }

    fn acquire_block(
        &mut self,
        size: usize,
        align: usize,
        offset: usize,
        stress_test: bool,
    ) -> Address {
        if self.handle_obvious_oom_request(self.tls, size) {
            return Address::ZERO;
        }

        let block_size = (size + BLOCK_MASK) & (!BLOCK_MASK);
        let acquired_start = self.get_space().acquire(
            self.tls,
            bytes_to_pages_up(block_size),
            self.get_context().get_alloc_options(),
        );
        if acquired_start.is_zero() {
            trace!("Failed to acquire a new block");
            acquired_start
        } else {
            trace!(
                "Acquired a new block of size {} with start address {}",
                block_size,
                acquired_start
            );
            if !stress_test {
                self.set_limit(acquired_start, acquired_start + block_size);
                self.alloc(size, align, offset)
            } else {
                // For a stress test, we artificially make the fastpath fail by
                // manipulating the limit as below.
                // The assumption here is that we use an address range such that
                // cursor > block_size always.
                self.bump_pointer
                    .reset_for_stress_test(acquired_start, block_size);
                // Note that we have just acquired a new block so we know that we don't have to go
                // through the entire allocation sequence again, we can directly call the slow path
                // allocation.
                self.alloc_slow_once_precise_stress(size, align, offset, false)
            }
        }
    }
}
