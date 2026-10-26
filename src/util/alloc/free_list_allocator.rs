// This is a free list allocator written based on Microsoft's mimalloc allocator https://www.microsoft.com/en-us/research/publication/mimalloc-free-list-sharding-in-action/

use std::sync::Arc;

use super::allocator::AllocatorContext;
use crate::policy::marksweepspace::native_ms::*;
use crate::util::alloc::allocator;
use crate::util::alloc::Allocator;
use crate::util::Address;
use crate::util::VMThread;
use crate::vm::VMBinding;

/// A MiMalloc free list allocator.
///
/// # `#[repr(C)]` Memory Layout & Visibility Note
/// `FreeListAllocator` is `#[repr(C)]` so VM bindings may embed `Allocators<VM>` (inside
/// [`crate::Mutator`]) in mutator thread-local storage. `space` uses niche-optimized
/// `Option<&'static MarkSweepSpace<VM>>` (`None` when unconfigured) and the four `BlockLists`
/// fields are module-private and `#[repr(transparent)]` over `Option<Box<[BlockList; 49]>>`:
///
/// | Field | Type | 64-bit Offset | 64-bit Size | 32-bit Offset | 32-bit Size |
/// | :--- | :--- | :---: | :---: | :---: | :---: |
/// | `tls` | [`VMThread`] | `0` | `8 B` | `0` | `4 B` |
/// | `space` | `Option<&'static MarkSweepSpace<VM>>` | `8` | `8 B` | `4` | `4 B` |
/// | `context` | `Arc<AllocatorContext<VM>>` | `16` | `8 B` | `8` | `4 B` |
/// | `available_blocks` | `BlockLists` | `24` | `8 B` | `12` | `4 B` |
/// | `available_blocks_stress` | `BlockLists` | `32` | `8 B` | `16` | `4 B` |
/// | `unswept_blocks` | `BlockLists` | `40` | `8 B` | `20` | `4 B` |
/// | `consumed_blocks` | `BlockLists` | `48` | `8 B` | `24` | `4 B` |
/// | **Inline Total** | `FreeListAllocator<VM>` | — | **`56 B` (align `8 B`)** | — | **`28 B` (align `4 B`)** |
#[repr(C)]
pub struct FreeListAllocator<VM: VMBinding> {
    /// [`VMThread`] associated with this allocator instance
    pub tls: VMThread,
    space: Option<&'static MarkSweepSpace<VM>>,
    context: Arc<AllocatorContext<VM>>,
    /// blocks with free space
    available_blocks: BlockLists,
    /// blocks with free space for precise stress GC
    available_blocks_stress: BlockLists,
    /// blocks that are marked, not swept
    unswept_blocks: BlockLists,
    /// full blocks
    consumed_blocks: BlockLists,
}

impl<VM: VMBinding> Allocator<VM> for FreeListAllocator<VM> {
    fn get_tls(&self) -> VMThread {
        self.tls
    }

    fn get_space(&self) -> &'static dyn crate::policy::space::Space<VM> {
        self.ms_space()
    }

    fn get_context(&self) -> &AllocatorContext<VM> {
        &self.context
    }

    #[inline(always)]
    fn alloc(&mut self, size: usize, align: usize, offset: usize) -> Address {
        debug_assert!(
            size <= MAX_BIN_SIZE,
            "Alloc request for {} bytes is too big.",
            size
        );
        debug_assert!(align <= VM::MAX_ALIGNMENT);
        debug_assert!(align >= VM::MIN_ALIGNMENT);

        let bin = mi_bin_bounded::<VM>(size, align);
        let cell = self.available_blocks.alloc_cell_from_head(bin);
        if !cell.is_zero() {
            debug_assert!(
                !(*self.context.options.precise_stress
                    && self.context.options.is_stress_test_gc_enabled())
            );

            let res = allocator::align_allocation::<VM>(cell, align, offset);
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

        self.alloc_from_local_or_slow(bin, size, align, offset)
    }

    fn alloc_slow_once(&mut self, size: usize, align: usize, offset: usize) -> Address {
        let bin = mi_bin_bounded::<VM>(size, align);
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
        use crate::util::linear_scan::Region;
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
        self.ms_space().abandon_blocks(
            &mut self.available_blocks,
            &mut self.available_blocks_stress,
            &mut self.consumed_blocks,
            &mut self.unswept_blocks,
        );
    }
}

impl<VM: VMBinding> FreeListAllocator<VM> {
    pub(in crate::util::alloc) fn new_uninit(
        tls: VMThread,
        context: Arc<AllocatorContext<VM>>,
    ) -> Self {
        FreeListAllocator {
            tls,
            space: None,
            context,
            available_blocks: BlockLists::uninit(),
            available_blocks_stress: BlockLists::uninit(),
            unswept_blocks: BlockLists::uninit(),
            consumed_blocks: BlockLists::uninit(),
        }
    }

    #[inline(always)]
    pub(in crate::util::alloc) fn is_initialized(&self) -> bool {
        self.space.is_some()
    }

    #[inline(always)]
    fn ms_space(&self) -> &'static MarkSweepSpace<VM> {
        self.space
            .expect("FreeListAllocator slot is not initialized")
    }

    pub(crate) fn new(
        tls: VMThread,
        space: &'static MarkSweepSpace<VM>,
        context: Arc<AllocatorContext<VM>>,
    ) -> Self {
        FreeListAllocator {
            tls,
            space: Some(space),
            context,
            available_blocks: new_empty_block_lists(),
            available_blocks_stress: new_empty_block_lists(),
            unswept_blocks: new_empty_block_lists(),
            consumed_blocks: new_empty_block_lists(),
        }
    }

    #[inline(never)]
    fn alloc_from_local_or_slow(
        &mut self,
        bin: BinIndex,
        size: usize,
        align: usize,
        offset: usize,
    ) -> Address {
        if self.find_free_block_local(bin) {
            let cell = self.available_blocks.alloc_cell_from_head(bin);
            if !cell.is_zero() {
                debug_assert!(
                    !(*self.context.options.precise_stress
                        && self.context.options.is_stress_test_gc_enabled())
                );
                let res = allocator::align_allocation::<VM>(cell, align, offset);
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

    fn find_free_block_stress(&mut self, bin: BinIndex, size: usize, align: usize) -> bool {
        Self::find_free_block_with(
            &mut self.available_blocks_stress,
            &mut self.consumed_blocks,
            bin,
        ) || self.recycle_local_blocks(bin, true)
            || self.acquire_global_block(bin, size, align, true)
    }

    fn find_free_block_local(&mut self, bin: BinIndex) -> bool {
        Self::find_free_block_with(&mut self.available_blocks, &mut self.consumed_blocks, bin)
            || self.recycle_local_blocks(bin, false)
    }

    fn find_free_block_with(
        available_blocks: &mut BlockLists,
        consumed_blocks: &mut BlockLists,
        bin: BinIndex,
    ) -> bool {
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

    fn add_to_available_blocks(&mut self, bin: BinIndex, block: OwnedBlock, stress: bool) {
        assert_eq!(
            block.id().get_state(),
            BlockState::Unmarked,
            "Cannot add unswept or unallocated block {:?} to available_blocks",
            block.id()
        );
        if stress {
            debug_assert!(*self.context.options.precise_stress);
            self.available_blocks_stress.push(bin, block);
        } else {
            self.available_blocks.push(bin, block);
        }
    }

    fn recycle_local_blocks(&mut self, bin: BinIndex, stress_test: bool) -> bool {
        if cfg!(feature = "eager_sweeping")
            || (!stress_test
                && *self.context.options.precise_stress
                && self.context.options.is_stress_test_gc_enabled())
        {
            return false;
        }
        loop {
            debug_assert!(if stress_test {
                self.available_blocks_stress[bin].is_empty()
            } else {
                self.available_blocks[bin].is_empty()
            });

            let Some(mut block) = self.unswept_blocks.pop(bin) else {
                return false;
            };

            block.sweep::<VM>();
            if block.has_free_cells() {
                self.add_to_available_blocks(bin, block, stress_test);
                return true;
            } else {
                self.consumed_blocks.push(bin, block);
            }
        }
    }

    fn acquire_global_block(
        &mut self,
        bin: BinIndex,
        size: usize,
        align: usize,
        stress_test: bool,
    ) -> bool {
        let space = self.ms_space();
        loop {
            match space.acquire_block(
                self.tls,
                size,
                align,
                self.get_context().get_alloc_options(),
            ) {
                BlockAcquireResult::Exhausted => {
                    debug!("Acquire global block: None");
                    return false;
                }

                BlockAcquireResult::Fresh(block) => {
                    debug!("Acquire global block: Fresh {:?}", block);
                    self.add_to_available_blocks(bin, block, stress_test);
                    return true;
                }

                BlockAcquireResult::AbandonedAvailable(block) => {
                    debug!("Acquire global block: AbandonedAvailable {:?}", block);
                    if block.has_free_cells() {
                        self.add_to_available_blocks(bin, block, stress_test);
                        return true;
                    } else {
                        self.consumed_blocks.push(bin, block);
                    }
                }

                BlockAcquireResult::AbandonedUnswept(mut block) => {
                    debug!("Acquire global block: AbandonedUnswept {:?}", block);
                    block.sweep::<VM>();
                    if block.has_free_cells() {
                        self.add_to_available_blocks(bin, block, stress_test);
                        return true;
                    } else {
                        self.consumed_blocks.push(bin, block);
                    }
                }
            }
        }
    }

    pub(crate) fn prepare(&mut self) {}

    pub(crate) fn release(&mut self) {
        let space = self.ms_space();
        for i in 0..MI_BIN_FULL {
            let bin = BinIndex::new(i).unwrap();

            if cfg!(feature = "eager_sweeping") {
                debug_assert!(self.unswept_blocks[bin].is_empty());
                self.available_blocks.release_blocks(bin, space);
                self.available_blocks_stress.release_blocks(bin, space);
                self.consumed_blocks.release_blocks(bin, space);
            } else {
                self.unswept_blocks.release_blocks(bin, space);
                self.available_blocks
                    .release_blocks_and_move_marked_to_unswept(
                        bin,
                        space,
                        &mut self.unswept_blocks,
                    );
                self.available_blocks_stress
                    .release_blocks_and_move_marked_to_unswept(
                        bin,
                        space,
                        &mut self.unswept_blocks,
                    );
                self.consumed_blocks
                    .release_blocks_and_move_marked_to_unswept(
                        bin,
                        space,
                        &mut self.unswept_blocks,
                    );
            }
        }

        space.abandon_blocks_in_gc(
            &mut self.available_blocks,
            &mut self.available_blocks_stress,
            &mut self.consumed_blocks,
            &mut self.unswept_blocks,
        );

        space.release_packet_done();
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
