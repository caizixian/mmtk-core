use super::gc_work::nursery_sweeping::{RCLazySweepNurseryBlocks, RCSTWSweepNurseryBlocks};
use crate::plan::concurrent::Pause;
use crate::policy::immix::block::{Block, BlockState};
use crate::policy::immix::{ImmixHooks, ImmixSpace};
use crate::scheduler::{GCWork, GCWorkScheduler, WorkBucketStage};
use crate::util::constants::LOG_BYTES_IN_PAGE;
use crate::util::linear_scan::Region;
use crate::vm::VMBinding;
use atomic::{Atomic, Ordering};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::RwLock;

struct BlockCache {
    cursor: AtomicUsize,
    buffer: RwLock<Vec<Atomic<Block>>>,
}

impl BlockCache {
    fn new() -> Self {
        Self {
            cursor: AtomicUsize::new(0),
            buffer: RwLock::new((0..32768).map(|_| Atomic::new(Block::ZERO)).collect()),
        }
    }

    fn len(&self) -> usize {
        self.cursor.load(Ordering::SeqCst)
    }

    fn push(&self, block: Block) {
        let i = self.cursor.fetch_add(1, Ordering::SeqCst);
        let buffer = self.buffer.read().unwrap();
        if i < buffer.len() {
            buffer[i].store(block, Ordering::SeqCst);
        } else {
            std::mem::drop(buffer);
            let mut buffer = self.buffer.write().unwrap();
            if i >= buffer.len() {
                buffer.resize_with(i << 1, || Atomic::new(Block::ZERO));
            }
            buffer[i].store(block, Ordering::Relaxed);
        }
    }

    fn visit_slice(&self, mut f: impl FnMut(&[Atomic<Block>])) {
        let count = self.cursor.load(Ordering::SeqCst);
        let blocks = self.buffer.read().unwrap();
        f(&blocks[0..count])
    }

    fn reset(&self) {
        self.cursor.store(0, Ordering::SeqCst);
    }
}

pub struct BlockAllocation<VM: VMBinding> {
    cm_in_progress_or_final_mark: AtomicBool,
    nursery_blocks: BlockCache,
    reused_blocks: BlockCache,
    _p: PhantomData<fn() -> VM>,
}

impl<VM: VMBinding> BlockAllocation<VM> {
    pub fn new() -> Self {
        Self {
            cm_in_progress_or_final_mark: AtomicBool::new(false),
            nursery_blocks: BlockCache::new(),
            reused_blocks: BlockCache::new(),
            _p: PhantomData,
        }
    }

    pub fn set_cm_in_progress_or_final_mark(&self, active: bool) {
        self.cm_in_progress_or_final_mark
            .store(active, Ordering::SeqCst);
    }

    pub fn clean_nursery_mb(&self) -> usize {
        self.nursery_blocks.len() << Block::LOG_BYTES >> 20
    }

    pub fn total_young_allocation_in_bytes(&self, space: &ImmixSpace<VM>) -> usize {
        (self.nursery_blocks.len() << Block::LOG_BYTES)
            + (space.get_mutator_recycled_lines_in_pages() << LOG_BYTES_IN_PAGE)
    }

    pub fn reset_block_mark_for_mutator_reused_blocks(&self, _pause: Pause) {
        // SATB sweep has problem scanning mutator recycled blocks.
        // Remaing the block state as "reusing" and reset them here.
        self.reused_blocks.visit_slice(|blocks| {
            for b in blocks {
                let b = b.load(Ordering::Relaxed);
                b.set_state(BlockState::Marked);
            }
        });
    }

    pub fn sweep_mutator_reused_blocks(&self, pause: Pause, mut on_block: impl FnMut(Block)) {
        if pause == Pause::Full || pause == Pause::FinalMark {
            self.reused_blocks.reset();
            return;
        }
        self.reused_blocks.visit_slice(|blocks| {
            for b in blocks {
                on_block(b.load(Ordering::Relaxed));
            }
        });
        self.reused_blocks.reset();
    }

    /// Reset allocated_block_buffer and free nursery blocks.
    pub fn sweep_nursery_blocks(
        &self,
        space: &ImmixSpace<VM>,
        max_stw_sweep_blocks: usize,
        pause: Pause,
    ) {
        const PARALLEL_STW_SWEEPING: bool = false;
        let scheduler = space.scheduler();
        self.nursery_blocks.visit_slice(|blocks| {
            if PARALLEL_STW_SWEEPING {
                return self.parallel_sweep_all_nursery_blocks(scheduler, blocks);
            }
            let total_nursery_blocks = blocks.len();
            let stw_limit = if pause == Pause::Full {
                total_nursery_blocks
            } else {
                usize::min(total_nursery_blocks, max_stw_sweep_blocks)
            };
            for b in &blocks[0..stw_limit] {
                let block = b.load(Ordering::Relaxed);
                debug_assert_ne!(block.get_state(), BlockState::Unallocated);
                block.rc_sweep_nursery(space);
            }
            if total_nursery_blocks > stw_limit {
                let packets = blocks[stw_limit..total_nursery_blocks]
                    .chunks(1024)
                    .map(|c| {
                        let blocks: Vec<Block> =
                            c.iter().map(|x| x.load(Ordering::Relaxed)).collect();
                        Box::new(RCLazySweepNurseryBlocks::new(blocks)) as Box<dyn GCWork<VM>>
                    })
                    .collect();
                scheduler.work_buckets[WorkBucketStage::Concurrent].bulk_add_deferred(packets);
            }
        });
        self.nursery_blocks.reset();
    }

    fn parallel_sweep_all_nursery_blocks(
        &self,
        scheduler: &GCWorkScheduler<VM>,
        blocks: &[Atomic<Block>],
    ) {
        let total_nursery_blocks = blocks.len();
        let packets = blocks[..total_nursery_blocks]
            .chunks(1024)
            .map(|c| {
                let blocks: Vec<Block> = c.iter().map(|x| x.load(Ordering::Relaxed)).collect();
                Box::new(RCSTWSweepNurseryBlocks::new(blocks)) as Box<dyn GCWork<VM>>
            })
            .collect();
        scheduler.work_buckets[WorkBucketStage::Unconstrained].bulk_add(packets);
    }
}

impl<VM: VMBinding> ImmixHooks<VM> for BlockAllocation<VM> {
    fn on_clean_block_acquired(&self, block: Block, copy: bool) {
        if !copy {
            self.nursery_blocks.push(block);
        }
        if copy {
            block.initialize_field_unlog_table_as_unlogged::<VM>();
        }
        if self.cm_in_progress_or_final_mark() {
            block.initialize_mark_table_as_marked::<VM>();
        } else {
            block.clear_mark_table::<VM>();
        }
    }

    fn on_reusable_block_acquired(&self, block: Block, copy: bool) {
        if !copy {
            self.reused_blocks.push(block);
        }
    }

    fn cm_in_progress_or_final_mark(&self) -> bool {
        self.cm_in_progress_or_final_mark.load(Ordering::SeqCst)
    }
}
