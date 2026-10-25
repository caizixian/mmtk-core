// This is a free list allocator written based on Microsoft's mimalloc allocator https://www.microsoft.com/en-us/research/publication/mimalloc-free-list-sharding-in-action/

#![deny(unsafe_op_in_unsafe_fn)]

use std::sync::Arc;

use super::allocator::AllocatorContext;
use crate::policy::marksweepspace::native_ms::*;
use crate::util::alloc::allocator;
use crate::util::alloc::Allocator;
use crate::util::linear_scan::Region;
use crate::util::Address;
use crate::util::VMThread;
use crate::vm::VMBinding;

/// A MiMalloc free list allocator.
///
/// # `#[repr(C)]` Memory Layout & Visibility Note
/// `FreeListAllocator` is `#[repr(C)]` so VM bindings may embed `Allocators<VM>` (inside
/// [`crate::Mutator`]) in mutator thread-local storage. Its internal `BlockLists` /
/// `UnsweptBlockLists` fields (`available_blocks`, `available_blocks_stress`, `unswept_blocks`,
/// and `consumed_blocks`) are module-private and `#[repr(transparent)]` over pointer-sized
/// `Box<[BlockList<B>; 49]>` handles:
///
/// | Field | Type | 64-bit Offset | 64-bit Size | 32-bit Offset | 32-bit Size |
/// | :--- | :--- | :---: | :---: | :---: | :---: |
/// | `tls` | [`VMThread`] | `0` | `8 B` | `0` | `4 B` |
/// | `space` | `&'static MarkSweepSpace<VM>` | `8` | `8 B` | `4` | `4 B` |
/// | `context` | `Arc<AllocatorContext<VM>>` | `16` | `8 B` | `8` | `4 B` |
/// | `available_blocks` | `BlockLists` | `24` | `8 B` | `12` | `4 B` |
/// | `available_blocks_stress` | `BlockLists` | `32` | `8 B` | `16` | `4 B` |
/// | `unswept_blocks` | `UnsweptBlockLists` | `40` | `8 B` | `20` | `4 B` |
/// | `consumed_blocks` | `BlockLists` | `48` | `8 B` | `24` | `4 B` |
/// | **Inline Total** | `FreeListAllocator<VM>` | — | **`56 B` (align `8 B`)** | — | **`28 B` (align `4 B`)** |
///
/// *(Heap footprint note: shrinking `BlockList` from `32 B` to `24 B` on 64-bit (`16 B -> 12 B`
/// on 32-bit) reduces the 4 boxed 49-bin arrays per `FreeListAllocator<VM>` from `6,272 B` to
/// `4,704 B` on 64-bit (`3,136 B -> 2,352 B` on 32-bit), saving `1,568 B` per mutator on 64-bit
/// while keeping inline `size_of::<FreeListAllocator<VM>>()` unchanged at `7 * size_of::<usize>()`.)*
#[repr(C)]
pub struct FreeListAllocator<VM: VMBinding> {
    /// [`VMThread`] associated with this allocator instance
    pub tls: VMThread,
    space: &'static MarkSweepSpace<VM>,
    context: Arc<AllocatorContext<VM>>,
    /// blocks with free space
    available_blocks: BlockLists,
    /// blocks with free space for precise stress GC
    /// For precise stress GC, we need to be able to trigger slowpath allocation for
    /// each allocation. To achieve this, we put available blocks to this list. So
    /// normal fastpath allocation will fail, as they will see the block lists
    /// as empty.
    available_blocks_stress: BlockLists,
    /// blocks that are marked, not swept
    unswept_blocks: UnsweptBlockLists,
    /// full blocks
    consumed_blocks: BlockLists,
}

impl<VM: VMBinding> Allocator<VM> for FreeListAllocator<VM> {
    fn get_tls(&self) -> VMThread {
        self.tls
    }

    fn get_space(&self) -> &'static dyn crate::policy::space::Space<VM> {
        self.space
    }

    fn get_context(&self) -> &AllocatorContext<VM> {
        &self.context
    }

    // Find a block with free space and allocate to it
    fn alloc(&mut self, size: usize, align: usize, offset: usize) -> Address {
        debug_assert!(
            size <= MAX_BIN_SIZE,
            "Alloc request for {} bytes is too big.",
            size
        );
        debug_assert!(align <= VM::MAX_ALIGNMENT);
        debug_assert!(align >= VM::MIN_ALIGNMENT);

        let bin = mi_bin_bounded::<VM>(size, align);
        if self.find_free_block_local(bin) {
            let cell = self.available_blocks.alloc_cell_from_head(bin);
            if !cell.is_zero() {
                // We succeeded in fastpath alloc, this cannot be precise stress test
                debug_assert!(
                    !(*self.context.options.precise_stress
                        && self.context.options.is_stress_test_gc_enabled())
                );

                let res = allocator::align_allocation::<VM>(cell, align, offset);
                // Make sure that the allocation region is within the cell
                #[cfg(debug_assertions)]
                {
                    let cell_size = bin_cell_size(bin);
                    debug_assert!(
                        res + size <= cell + cell_size,
                        "Allocating (size = {}, align = {}, offset = {}) to the cell {} of size {}, but the end of the allocation region {} is beyond the cell end {}",
                        size, align, offset, cell, cell_size, res + size, cell + cell_size
                    );
                }
                return res;
            }
        }

        self.alloc_slow(size, align, offset)
    }

    fn alloc_slow_once(&mut self, size: usize, align: usize, offset: usize) -> Address {
        let bin = mi_bin_bounded::<VM>(size, align);
        // Try to get a block from the space
        if self.acquire_global_block(bin, size, align, false) {
            let addr = self.available_blocks.alloc_cell_from_head(bin);
            debug_assert!(!addr.is_zero());
            allocator::align_allocation::<VM>(addr, align, offset)
        } else {
            Address::ZERO
        }
    }

    fn does_thread_local_allocation(&self) -> bool {
        true
    }

    fn get_thread_local_buffer_granularity(&self) -> usize {
        Block::BYTES
    }

    fn alloc_slow_once_precise_stress(
        &mut self,
        size: usize,
        align: usize,
        offset: usize,
        need_poll: bool,
    ) -> Address {
        trace!("allow slow precise stress s={}", size);
        if need_poll {
            let poll_bin = mi_bin_bounded::<VM>(0, 0);
            self.acquire_global_block(poll_bin, 0, 0, true);
        }

        // mimic what fastpath allocation does, except that we allocate from available_blocks_stress.
        let bin = mi_bin_bounded::<VM>(size, align);
        if self.find_free_block_stress(bin, size, align) {
            let cell = self.available_blocks_stress.alloc_cell_from_head(bin);
            debug_assert!(!cell.is_zero());
            allocator::align_allocation::<VM>(cell, align, offset)
        } else {
            Address::ZERO
        }
    }

    fn on_mutator_destroy(&mut self) {
        let mut global = self.space.get_abandoned_block_lists().lock().unwrap();
        self.abandon_blocks(&mut global);
    }
}

impl<VM: VMBinding> FreeListAllocator<VM> {
    // New free list allocator
    pub(crate) fn new(
        tls: VMThread,
        space: &'static MarkSweepSpace<VM>,
        context: Arc<AllocatorContext<VM>>,
    ) -> Self {
        FreeListAllocator {
            tls,
            space,
            context,
            available_blocks: new_empty_block_lists(),
            available_blocks_stress: new_empty_block_lists(),
            unswept_blocks: new_empty_unswept_block_lists(),
            consumed_blocks: new_empty_block_lists(),
        }
    }

    // Find an available block when precise stress GC is enabled. This includes getting a block from the space.
    fn find_free_block_stress(&mut self, bin: BinIndex, size: usize, align: usize) -> bool {
        Self::find_free_block_with(
            &mut self.available_blocks_stress,
            &mut self.consumed_blocks,
            bin,
        ) || self.recycle_local_blocks(bin, true)
            || self.acquire_global_block(bin, size, align, true)
    }

    // Find an available block from local block lists
    fn find_free_block_local(&mut self, bin: BinIndex) -> bool {
        Self::find_free_block_with(&mut self.available_blocks, &mut self.consumed_blocks, bin)
            || self.recycle_local_blocks(bin, false)
    }

    // Ensure the front block of `available_blocks[bin]` has at least one free cell, moving
    // exhausted blocks to `consumed_blocks[bin]`. Returns `true` iff a block with free cells is
    // now at the front of `available_blocks[bin]`.
    fn find_free_block_with(
        available_blocks: &mut BlockLists,
        consumed_blocks: &mut BlockLists,
        bin: BinIndex,
    ) -> bool {
        debug_assert_eq!(available_blocks[bin].size(), bin_cell_size(bin));

        while !available_blocks[bin].is_empty() {
            if available_blocks[bin].has_head_free_cells() {
                return true;
            }
            let full_block = available_blocks.pop(bin).unwrap();
            consumed_blocks.push(bin, full_block);
        }

        debug_assert!(available_blocks[bin].is_empty());
        false
    }

    /// Add `block` to `bin` in `available_blocks` (or `available_blocks_stress` when `stress` is `true`).
    fn add_to_available_blocks(&mut self, bin: BinIndex, block: SweptBlock, stress: bool) {
        if stress {
            debug_assert!(*self.context.options.precise_stress);
            self.available_blocks_stress.push(bin, block);
        } else {
            self.available_blocks.push(bin, block);
        }
    }

    /// Try to sweep and recycle local unswept blocks for `bin`. This is a no-op for eager sweeping.
    fn recycle_local_blocks(&mut self, bin: BinIndex, stress_test: bool) -> bool {
        if cfg!(feature = "eager_sweeping") {
            // We have swept blocks in the last GC. If we run out of available blocks, there is nothing we can do.
            false
        } else if !stress_test
            && *self.context.options.precise_stress
            && self.context.options.is_stress_test_gc_enabled()
        {
            // Under precise stress GC, fastpath must fall through to slowpath so every allocation
            // checks `should_do_stress_gc()`.
            false
        } else {
            // Pop blocks from `unswept_blocks` and linearly sweep each into a `SweptBlock`.
            loop {
                debug_assert!(if stress_test {
                    self.available_blocks_stress[bin].is_empty()
                } else {
                    self.available_blocks[bin].is_empty()
                });

                let Some(unswept) = self.unswept_blocks.pop(bin) else {
                    return false;
                };

                let swept = unswept.sweep::<VM>();
                if swept.has_free_cells() {
                    self.add_to_available_blocks(bin, swept, stress_test);
                    return true;
                } else {
                    // Nothing was freed from this block.
                    self.consumed_blocks.push(bin, swept);
                }
            }
        }
    }

    /// Acquire a block for `bin` from `self.space` and push it onto the target available list.
    fn acquire_global_block(
        &mut self,
        bin: BinIndex,
        size: usize,
        align: usize,
        stress_test: bool,
    ) -> bool {
        loop {
            match self.space.acquire_block(
                self.tls,
                size,
                align,
                self.get_context().get_alloc_options(),
            ) {
                BlockAcquireResult::Exhausted => {
                    debug!("Acquire global block: None");
                    // GC
                    return false;
                }

                BlockAcquireResult::Fresh(fresh) => {
                    debug!("Acquire global block: Fresh {:?}", fresh);
                    let swept = fresh.init(bin, self.tls);
                    self.add_to_available_blocks(bin, swept, stress_test);
                    return true;
                }

                BlockAcquireResult::AbandonedAvailable(mut swept) => {
                    debug!("Acquire global block: AbandonedAvailable {:?}", swept);
                    swept.set_tls(self.tls);
                    if swept.has_free_cells() {
                        self.add_to_available_blocks(bin, swept, stress_test);
                        return true;
                    } else {
                        self.consumed_blocks.push(bin, swept);
                    }
                }

                BlockAcquireResult::AbandonedUnswept(mut unswept) => {
                    debug!("Acquire global block: AbandonedUnswep {:?}", unswept);
                    unswept.set_tls(self.tls);
                    let swept = unswept.sweep::<VM>();
                    if swept.has_free_cells() {
                        self.add_to_available_blocks(bin, swept, stress_test);
                        return true;
                    } else {
                        self.consumed_blocks.push(bin, swept);
                    }
                }
            }
        }
    }

    pub(crate) fn prepare(&mut self) {}

    pub(crate) fn release(&mut self) {
        for i in 0..MI_BIN_FULL {
            let bin = BinIndex::new(i).unwrap();

            if cfg!(feature = "eager_sweeping") {
                // If we do eager sweeping, we should have no unswept blocks.
                debug_assert!(self.unswept_blocks[bin].is_empty());
                // For eager sweeping, release unmarked blocks and leave marked blocks to be swept
                // later in `SweepChunk` work packets.
                self.available_blocks.release_blocks(bin, self.space);
                self.available_blocks_stress.release_blocks(bin, self.space);
                self.consumed_blocks.release_blocks(bin, self.space);
            } else {
                self.unswept_blocks.release_blocks(bin, self.space);
                // For lazy sweeping, release unmarked blocks and move surviving marked blocks from
                // available and consumed into `unswept_blocks`.
                self.available_blocks
                    .release_blocks_and_move_marked_to_unswept(
                        bin,
                        self.space,
                        &mut self.unswept_blocks,
                    );
                self.available_blocks_stress
                    .release_blocks_and_move_marked_to_unswept(
                        bin,
                        self.space,
                        &mut self.unswept_blocks,
                    );
                self.consumed_blocks
                    .release_blocks_and_move_marked_to_unswept(
                        bin,
                        self.space,
                        &mut self.unswept_blocks,
                    );
            }
        }

        // We abandon block lists immediately.  Otherwise, some mutators will hold lots of blocks
        // locally and prevent other mutators to use.
        {
            let mut global = self.space.get_abandoned_block_lists_in_gc().lock().unwrap();
            self.abandon_blocks(&mut global);
        }

        self.space.release_packet_done();
    }

    fn abandon_blocks(&mut self, global: &mut AbandonedBlockLists) {
        global.abandon_from(
            &mut self.available_blocks,
            &mut self.available_blocks_stress,
            &mut self.consumed_blocks,
            &mut self.unswept_blocks,
        );
    }
}

#[cfg(all(test, feature = "mock_test"))]
mod tests {
    use super::*;
    use crate::util::test_util::mock_vm::MockVM;
    use std::mem::{align_of, offset_of, size_of};

    #[test]
    fn test_free_list_allocator_repr_c_layout() {
        type Alloc = FreeListAllocator<MockVM>;
        let word = size_of::<usize>();
        assert_eq!(size_of::<Alloc>(), 7 * word);
        assert_eq!(align_of::<Alloc>(), word);
        assert_eq!(offset_of!(Alloc, tls), 0);
        assert_eq!(offset_of!(Alloc, space), word);
        assert_eq!(offset_of!(Alloc, context), 2 * word);
        assert_eq!(offset_of!(Alloc, available_blocks), 3 * word);
        assert_eq!(offset_of!(Alloc, available_blocks_stress), 4 * word);
        assert_eq!(offset_of!(Alloc, unswept_blocks), 5 * word);
        assert_eq!(offset_of!(Alloc, consumed_blocks), 6 * word);
    }
}
