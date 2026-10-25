// This is a free list allocator written based on Microsoft's mimalloc allocator https://www.microsoft.com/en-us/research/publication/mimalloc-free-list-sharding-in-action/

use std::sync::Arc;

use super::allocator::AllocatorContext;
use crate::policy::marksweepspace::native_ms::*;
use crate::util::alloc::allocator;
use crate::util::alloc::Allocator;
use crate::util::linear_scan::Region;
use crate::util::Address;
use crate::util::VMThread;
use crate::vm::VMBinding;

/// A MiMalloc free list allocator
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
    unswept_blocks: BlockLists,
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

        if let Some(block) = self.find_free_block_local(size, align) {
            // SAFETY: `find_free_block_local` returned an initialized `MarkSweepSpace` block
            // owned exclusively by `self` with a well-formed intrusive free list.
            let cell = unsafe { self.block_alloc(block) };
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
                    let cell_size = block.load_block_cell_size();
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
        // Try get a block from the space
        if let Some(block) = self.acquire_global_block(size, align, false) {
            // SAFETY: `acquire_global_block` returned an initialized `MarkSweepSpace` block
            // owned exclusively by `self` with a well-formed intrusive free list.
            let addr = unsafe { self.block_alloc(block) };
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
            self.acquire_global_block(0, 0, true);
        }

        // mimic what fastpath allocation does, except that we allocate from available_blocks_stress.
        if let Some(block) = self.find_free_block_stress(size, align) {
            // SAFETY: `find_free_block_stress` returned an initialized `MarkSweepSpace` block
            // owned exclusively by `self` with a well-formed intrusive free list.
            let cell = unsafe { self.block_alloc(block) };
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
    // New free list allcoator
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
            unswept_blocks: new_empty_block_lists(),
            consumed_blocks: new_empty_block_lists(),
        }
    }

    /// Find and pop a free cell within `block`, zeroing the cell before returning it.
    ///
    /// # Safety
    /// `block` must be an initialized `MarkSweepSpace` block exclusively owned by `self`, whose
    /// `FREE_LIST_TABLE` head (if non-zero) points to a well-formed intrusive free list of
    /// word-aligned cells of size `block.load_block_cell_size()` within `[block.start(), block.end())`.
    unsafe fn block_alloc(&mut self, block: Block) -> Address {
        // SAFETY: Caller guarantees `block` is an initialized block exclusively owned by `self`.
        let cell = unsafe { block.load_free_list() };
        if cell.is_zero() {
            return cell; // return failed allocation
        }
        // SAFETY: Caller guarantees non-zero `cell` is a valid word-aligned free cell inside
        // `block` whose first word holds the next intrusive free-list link.
        let next_cell = unsafe { cell.load::<Address>() };
        debug_assert!(
            next_cell.is_zero() || block.includes_address(next_cell),
            "next_cell {} is not in {:?}",
            next_cell,
            block
        );
        // SAFETY: Caller guarantees `block` is exclusively owned by `self` with mapped metadata.
        unsafe { block.store_free_list(next_cell) };

        // Zeroing memory right before we return it (which also clears the intrusive free-list link
        // stored in the first word of `cell`).
        // If we move the zeroing to somewhere else, we need to clear the list link here: cell.store::<Address>(Address::ZERO)
        let cell_size = block.load_block_cell_size();
        crate::util::memory::zero(cell, cell_size);

        // Make sure the memory is zeroed. This looks silly as we zero the cell right before this check.
        // But we would need to move the zeroing to somewhere so we can do zeroing at a coarser grainularity.
        #[cfg(debug_assertions)]
        {
            let mut cursor = cell;
            while cursor < cell + cell_size {
                // SAFETY: `cursor` is word-aligned (`cell` and `cell_size` are word-aligned) and
                // lies within the freshly zeroed `[cell, cell + cell_size)` cell owned by `self`.
                debug_assert_eq!(unsafe { cursor.load::<usize>() }, 0);
                cursor += crate::util::constants::BYTES_IN_ADDRESS;
            }
        }

        cell
    }

    // Find an available block when stress GC is enabled. This includes getting a block from the space.
    fn find_free_block_stress(&mut self, size: usize, align: usize) -> Option<Block> {
        Self::find_free_block_with(
            &mut self.available_blocks_stress,
            &mut self.consumed_blocks,
            size,
            align,
        )
        .or_else(|| self.recycle_local_blocks(size, align, true))
        .or_else(|| self.acquire_global_block(size, align, true))
    }

    // Find an available block from local block lists
    fn find_free_block_local(&mut self, size: usize, align: usize) -> Option<Block> {
        Self::find_free_block_with(
            &mut self.available_blocks,
            &mut self.consumed_blocks,
            size,
            align,
        )
        .or_else(|| self.recycle_local_blocks(size, align, false))
    }

    // Find an available block
    // This will usually be the first block on the available list. If all available blocks are found
    // to be full, other lists are searched
    // This function allows different available block lists -- normal allocation uses self.avaialble_blocks, and precise stress test uses self.avialable_blocks_stress.
    fn find_free_block_with(
        available_blocks: &mut BlockLists,
        consumed_blocks: &mut BlockLists,
        size: usize,
        align: usize,
    ) -> Option<Block> {
        let bin = mi_bin::<VM>(size, align);
        debug_assert!(bin <= MAX_BIN);

        let available = &mut available_blocks[bin];
        debug_assert!(available.size >= size);

        while let Some(block) = available.first() {
            // SAFETY: `block` is in `available`, which is exclusively owned here and contains
            // only initialized `MarkSweepSpace` blocks with mapped `FREE_LIST_TABLE` metadata.
            if unsafe { block.has_free_cells() } {
                return Some(block);
            }
            let popped = available.pop();
            debug_assert_eq!(popped, Some(block));
            // SAFETY: `block` was just popped from `available`, so it is an initialized
            // `MarkSweepSpace` block with mapped side metadata and belongs to no `BlockList`.
            unsafe { consumed_blocks.get_mut(bin).unwrap().push(block) };
        }

        debug_assert!(available_blocks[bin].is_empty());
        None
    }

    /// Add a block to the given bin in the available block lists. Depending on which available block list we are using, this
    /// method may add the block to available_blocks, or available_blocks_stress.
    ///
    /// # Safety
    /// `bin` must be `<= MAX_BIN`, and `block` must be an initialized `MarkSweepSpace` block with
    /// mapped local side metadata that does not currently belong to any `BlockList`.
    unsafe fn add_to_available_blocks(&mut self, bin: usize, block: Block, stress: bool) {
        if stress {
            debug_assert!(*self.context.options.precise_stress);
            // SAFETY: Caller guarantees `block` is initialized, has mapped local side metadata,
            // and belongs to no `BlockList`.
            unsafe { self.available_blocks_stress[bin].push(block) };
        } else {
            // SAFETY: Same as above.
            unsafe { self.available_blocks[bin].push(block) };
        }
    }

    /// Tries to recycle local blocks if there is any. This is a no-op for eager sweeping mark sweep.
    fn recycle_local_blocks(
        &mut self,
        size: usize,
        align: usize,
        _stress_test: bool,
    ) -> Option<Block> {
        if cfg!(feature = "eager_sweeping") {
            // We have swept blocks in the last GC. If we run out of available blocks, there is nothing we can do.
            None
        } else {
            // Get blocks from unswept_blocks and attempt to sweep
            loop {
                let bin = mi_bin::<VM>(size, align);
                debug_assert!(self.available_blocks[bin].is_empty()); // only use this function if there are no blocks available

                let block = self.unswept_blocks.get_mut(bin).unwrap().pop()?;

                // SAFETY: `block` was popped from this allocator's `unswept_blocks`, so it is an
                // initialized `MarkSweepSpace` block with mapped metadata/cells exclusively owned
                // by `self` outside GC tracing.
                unsafe { block.sweep::<VM>() };
                // SAFETY: `block` is exclusively owned by `self` with mapped `FREE_LIST_TABLE`.
                if unsafe { block.has_free_cells() } {
                    // recyclable block
                    let stress = self.context.options.is_stress_test_gc_enabled();
                    // SAFETY: `block` was popped from `unswept_blocks`, is initialized, and
                    // belongs to no `BlockList`.
                    unsafe { self.add_to_available_blocks(bin, block, stress) };
                    return Some(block);
                } else {
                    // nothing was freed from this block
                    // SAFETY: `block` was popped from `unswept_blocks`, is initialized, and
                    // belongs to no `BlockList`.
                    unsafe { self.consumed_blocks.get_mut(bin).unwrap().push(block) };
                }
            }
        }
    }

    /// Get a block from the space.
    fn acquire_global_block(
        &mut self,
        size: usize,
        align: usize,
        stress_test: bool,
    ) -> Option<Block> {
        let bin = mi_bin::<VM>(size, align);
        loop {
            match self.space.acquire_block(
                self.tls,
                size,
                align,
                self.get_context().get_alloc_options(),
            ) {
                crate::policy::marksweepspace::native_ms::BlockAcquireResult::Exhausted => {
                    debug!("Acquire global block: None");
                    // GC
                    return None;
                }

                crate::policy::marksweepspace::native_ms::BlockAcquireResult::Fresh(block) => {
                    debug!("Acquire global block: Fresh {:?}", block);
                    let cell_size = self.available_blocks[bin].size;
                    // SAFETY: `block` was freshly acquired from `self.space` with mapped memory
                    // and local side metadata, exclusively owned by `self`; `cell_size` is a
                    // non-zero word-aligned MiMalloc bin size (`<= Block::BYTES`).
                    unsafe { self.init_block(block, cell_size) };
                    // SAFETY: `block` is now initialized and belongs to no `BlockList`.
                    unsafe { self.add_to_available_blocks(bin, block, stress_test) };

                    return Some(block);
                }

                crate::policy::marksweepspace::native_ms::BlockAcquireResult::AbandonedAvailable(
                    block,
                ) => {
                    debug!("Acquire global block: AbandonedAvailable {:?}", block);
                    // SAFETY: `block` was popped from `self.space.abandoned.available`, so it is
                    // an initialized `MarkSweepSpace` block with mapped metadata, belongs to no
                    // `BlockList`, and is now exclusively owned by `self`.
                    unsafe { block.store_tls(self.tls) };
                    // SAFETY: `block` is exclusively owned by `self` with mapped `FREE_LIST_TABLE`.
                    if unsafe { block.has_free_cells() } {
                        // SAFETY: `block` is initialized and belongs to no `BlockList`.
                        unsafe { self.add_to_available_blocks(bin, block, stress_test) };
                        return Some(block);
                    } else {
                        // SAFETY: `block` is initialized and belongs to no `BlockList`.
                        unsafe { self.consumed_blocks[bin].push(block) };
                    }
                }

                crate::policy::marksweepspace::native_ms::BlockAcquireResult::AbandonedUnswept(
                    block,
                ) => {
                    debug!("Acquire global block: AbandonedUnswep {:?}", block);
                    // SAFETY: `block` was popped from `self.space.abandoned.unswept`, so it is an
                    // initialized `MarkSweepSpace` block with mapped metadata/cells, belongs to no
                    // `BlockList`, and is now exclusively owned by `self` outside GC tracing.
                    unsafe {
                        block.store_tls(self.tls);
                        block.sweep::<VM>();
                    }
                    // SAFETY: `block` is exclusively owned by `self` with mapped `FREE_LIST_TABLE`.
                    if unsafe { block.has_free_cells() } {
                        // SAFETY: `block` is initialized and belongs to no `BlockList`.
                        unsafe { self.add_to_available_blocks(bin, block, stress_test) };
                        return Some(block);
                    } else {
                        // SAFETY: `block` is initialized and belongs to no `BlockList`.
                        unsafe { self.consumed_blocks[bin].push(block) };
                    }
                }
            }
        }
    }

    /// Initialize a freshly acquired block's intrusive free list and local side metadata.
    ///
    /// # Safety
    /// - `block` must be a freshly acquired block in `self.space` whose data pages
    ///   `[block.start(), block.end())` and local side metadata are mapped and exclusively owned
    ///   by `self`.
    /// - `cell_size` must be a non-zero multiple of `BYTES_IN_ADDRESS` and `<= Block::BYTES`.
    unsafe fn init_block(&self, block: Block, cell_size: usize) {
        debug_assert_ne!(cell_size, 0);
        self.space.record_new_block(block);

        // construct free list
        let block_end = block.start() + Block::BYTES;
        let mut old_cell = Address::ZERO;
        let mut new_cell = block.start();

        let final_cell = loop {
            // SAFETY: `new_cell` is word-aligned (`block.start()` is 64KB-aligned and `cell_size`
            // is word-aligned), `[new_cell, new_cell + cell_size)` lies within mapped block memory
            // `[block.start(), block_end)`, `cell_size >= BYTES_IN_ADDRESS`, and `block` is
            // exclusively owned by `self`.
            unsafe {
                new_cell.store::<Address>(old_cell);
            }
            old_cell = new_cell;
            new_cell += cell_size;
            if new_cell + cell_size > block_end {
                break old_cell;
            };
        };

        // SAFETY: `block` has mapped local side metadata and is exclusively owned by `self`.
        unsafe {
            block.store_free_list(final_cell);
            block.store_block_cell_size(cell_size);
        }
        #[cfg(feature = "malloc_native_mimalloc")]
        // SAFETY: Same as above (`LOCAL_FREE_LIST_TABLE` and `THREAD_FREE_LIST_TABLE` are mapped
        // and exclusively owned during fresh block initialization).
        unsafe {
            block.store_local_free_list(Address::ZERO);
            block.store_thread_free_list(Address::ZERO);
        }

        // SAFETY: `block` has mapped `TLS_TABLE` metadata and is exclusively owned by `self`.
        unsafe { self.store_block_tls(block) };
    }

    /// Free an allocated cell at `addr` back to its owning block's local free list.
    ///
    /// # Safety
    /// `addr` must be the non-zero, word-aligned start address of a live, exclusively freed cell
    /// of size `>= BYTES_IN_ADDRESS` allocated from a block in `self.space` with mapped cell memory
    /// and local side metadata.
    #[cfg(feature = "malloc_native_mimalloc")]
    unsafe fn free(&self, addr: Address) {
        assert!(!addr.is_zero(), "Attempted to free zero address.");

        use crate::util::ObjectReference;
        let block = Block::from_unaligned_address(addr);
        // SAFETY: Caller guarantees `addr` lies in an allocated block of `self.space` with mapped
        // `TLS_TABLE` metadata.
        let block_tls = unsafe { block.load_tls() };

        if self.tls == block_tls {
            // same thread that allocated
            // SAFETY: `self.tls == block_tls` guarantees this thread owns `block`'s thread-local
            // free list (`LOCAL_FREE_LIST_TABLE`), and caller guarantees `addr` is a word-aligned
            // freed cell with mapped memory exclusively freed here.
            unsafe {
                let local_free = block.load_local_free_list();
                addr.store(local_free);
                block.store_local_free_list(addr);
            }
        } else {
            // different thread to allocator
            unreachable!(
                "tlss don't match freeing from block {}, my tls = {:?}, block tls = {:?}",
                block.start(),
                self.tls,
                block_tls
            );

            // I am not sure whether the following code would be used to free a block for other thread. I will just keep it here as commented out.
            // let mut success = false;
            // while !success {
            //     let thread_free = FreeListAllocator::<VM>::load_thread_free_list(block);
            //     unsafe {
            //         addr.store(thread_free);
            //     }
            //     success = FreeListAllocator::<VM>::cas_thread_free_list(&self, block, thread_free, addr);
            // }
        }

        // unset allocation bit
        // Note: We cannot use `unset_vo_bit_unsafe` because two threads may attempt to free
        // objects at adjacent addresses, and they may share the same byte in the VO bit metadata.
        // SAFETY: `assert!(!addr.is_zero())` above guarantees `addr` is non-zero, and caller
        // guarantees `addr` is an allocated object/cell address in `self.space` with mapped VO bits.
        crate::util::metadata::vo_bit::unset_vo_bit(unsafe {
            ObjectReference::from_raw_address_unchecked(addr)
        })
    }

    /// Store `self.tls` into `block`'s `TLS_TABLE`.
    ///
    /// # Safety
    /// `block` must belong to `self.space` with mapped `TLS_TABLE` metadata and exclusive access.
    unsafe fn store_block_tls(&self, block: Block) {
        // SAFETY: Caller guarantees `block` has mapped `TLS_TABLE` metadata and exclusive access.
        unsafe { block.store_tls(self.tls) };
    }

    pub(crate) fn prepare(&mut self) {}

    pub(crate) fn release(&mut self) {
        for bin in 0..MI_BIN_FULL {
            let unswept = self.unswept_blocks.get_mut(bin).unwrap();

            // If we do eager sweeping, we should have no unswept blocks.
            #[cfg(feature = "eager_sweeping")]
            debug_assert!(unswept.is_empty());

            let mut sweep_later = |list: &mut BlockList| {
                // SAFETY: Every block in `self`'s local lists belongs to `self.space`, and
                // `release` runs exclusively for this mutator during the STW `Release` stage
                // before `SweepChunk` packets run (`pending_release_packets` barrier).
                unsafe { list.release_blocks(self.space) };

                // For eager sweeping, that's it.  We just release unmarked blocks, and leave marked
                // blocks to be swept later in the `SweepChunk` work packet.

                // For lazy sweeping, we move blocks from available and consumed to unswept.  When
                // an allocator tries to use them, they will sweep the block.
                if cfg!(not(feature = "eager_sweeping")) {
                    unswept.append(list);
                }
            };

            sweep_later(&mut self.available_blocks[bin]);
            sweep_later(&mut self.available_blocks_stress[bin]);
            sweep_later(&mut self.consumed_blocks[bin]);
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
        // SAFETY: All blocks in `self`'s private block lists were acquired from `self.space`,
        // and `global` is locked from `self.space`.
        unsafe {
            global.abandon_from(
                &mut self.available_blocks,
                &mut self.available_blocks_stress,
                &mut self.consumed_blocks,
                &mut self.unswept_blocks,
            );
        }
    }
}
