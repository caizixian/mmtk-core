use std::sync::Mutex;
use std::{cell::UnsafeCell, marker::PhantomData};

use crate::plan::concurrent::Pause;
use crate::plan::global::Plan;
use crate::plan::lxr::gc_work::mature_evac::SelectDefragBlocks;
use crate::plan::lxr::gc_work::mature_evac::SELECT_DEFRAG_BLOCK_JOB_COUNTER;
use crate::policy::immix::block::{Block, BlockState};
use crate::policy::immix::line::Line;
use crate::policy::immix::ImmixSpace;
use crate::policy::space::Space;
use crate::scheduler::WorkBucketStage;
use crate::util::linear_scan::Region;
use crate::util::metadata::side_metadata::spec_defs::{IX_LINE_REUSE_COUNT, LOS_PAGE_REUSE_COUNT};
use crate::util::ObjectReference;
use crate::{
    plan::lxr::LXR,
    scheduler::GCWork,
    vm::{slot::Slot, VMBinding},
};

use super::gc_work::mature_evac::EvacuateMatureObjects;
use crate::util::constants::LOG_BYTES_IN_PAGE;
use atomic::Ordering;
use crossbeam::queue::SegQueue;
use std::sync::atomic::AtomicUsize;

#[repr(C)]
pub struct RemSetEntry<VM: VMBinding>(VM::VMSlot, u8);

impl<VM: VMBinding> RemSetEntry<VM> {
    fn encode(slot: VM::VMSlot, ix: bool) -> Self {
        let reuse = if ix {
            IX_LINE_REUSE_COUNT.load_atomic::<u8>(slot.to_address(), Ordering::SeqCst)
        } else {
            LOS_PAGE_REUSE_COUNT.load_atomic::<u8>(slot.to_address(), Ordering::SeqCst)
        };
        Self(slot, reuse)
    }

    pub fn decode(&self) -> (VM::VMSlot, u8) {
        (self.0, self.1)
    }
}

pub struct MatureEvecRemSet<VM: VMBinding> {
    gc_buffers: Vec<UnsafeCell<Vec<RemSetEntry<VM>>>>,
    pub global_packets: Mutex<Vec<Box<dyn GCWork<VM>>>>,
    local_packets: Vec<UnsafeCell<Vec<Box<dyn GCWork<VM>>>>>,
    _p: PhantomData<fn() -> VM>,
    size: AtomicUsize,
}

// SAFETY: `global_packets` (`Mutex<Vec<Box<dyn GCWork<VM>>>>`) and `size` (`AtomicUsize`) are
// intrinsically `Sync`. The per-worker `UnsafeCell` slots in `gc_buffers` and `local_packets` are
// indexed exclusively by `worker.ordinal` via `unsafe fn record` (whose safety contract requires a
// `GCWorker<VM>` belonging to the owning `MMTK<VM>` instance, with `&mut GCWorker<VM>` statically
// preventing re-entrant access on that worker) while bulk draining via `flush_all(&mut self)`
// requires `&mut MatureEvecRemSet<VM>` during single-worker STW `LXR::prepare`.
unsafe impl<VM: VMBinding> Sync for MatureEvecRemSet<VM> {}

impl<VM: VMBinding> MatureEvecRemSet<VM> {
    pub fn new(workers: usize) -> Self {
        let mut rs = Self {
            gc_buffers: vec![],
            global_packets: Mutex::new(vec![]),
            local_packets: vec![],
            _p: PhantomData,
            size: AtomicUsize::new(0),
        };
        rs.gc_buffers
            .resize_with(workers, || UnsafeCell::new(vec![]));
        rs.local_packets
            .resize_with(workers, || UnsafeCell::new(vec![]));
        rs
    }

    pub fn flush_all(&mut self) {
        let mature_evac_remsets = self.global_packets.get_mut().unwrap();
        self.size.store(0, Ordering::SeqCst);
        for cell in &mut self.gc_buffers {
            let buf = cell.get_mut();
            if !buf.is_empty() {
                let remset = std::mem::take(buf);
                mature_evac_remsets.push(Box::new(EvacuateMatureObjects::new(remset)));
            }
        }
        for cell in &mut self.local_packets {
            let buf = cell.get_mut();
            if !buf.is_empty() {
                let packets = std::mem::take(buf);
                for p in packets {
                    mature_evac_remsets.push(p);
                }
            }
        }
    }

    pub fn take_global_packets(&self) -> Vec<Box<dyn GCWork<VM>>> {
        let mut mature_evac_remsets = self.global_packets.lock().unwrap();
        std::mem::take(&mut *mature_evac_remsets)
    }

    /// Flush worker `id`'s local remembered-set buffer into `self.local_packets[id]`.
    ///
    /// # Safety
    ///
    /// - `id` must be in bounds (`id < self.gc_buffers.len()`, which equals `self.local_packets.len()`).
    /// - The caller must have exclusive access to `self.gc_buffers[id]` and `self.local_packets[id]`
    ///   for the duration of this call (no other live shared or mutable references to either cell's
    ///   contents may exist concurrently on this or any other thread).
    #[cold]
    unsafe fn flush(&self, id: usize) {
        let remset = {
            // SAFETY: Caller guarantees `id < self.gc_buffers.len()` and exclusive access to
            // `self.gc_buffers[id]`. `buf` is dropped at the end of this block before accessing
            // `self.local_packets[id]`.
            let buf = unsafe { &mut *self.gc_buffers[id].get() };
            if buf.is_empty() {
                return;
            }
            std::mem::take(buf)
        };
        self.size.fetch_add(remset.len(), Ordering::SeqCst);
        let w = EvacuateMatureObjects::new(remset);
        // SAFETY: Caller guarantees `id < self.local_packets.len()` and exclusive access to
        // `self.local_packets[id]`.
        let packet_buffer = unsafe { &mut *self.local_packets[id].get() };
        packet_buffer.push(Box::new(w));
    }

    /// Record slot `s` into `worker`'s thread-local mature-evacuation remembered-set buffer.
    ///
    /// # Safety
    ///
    /// `worker` must belong to the same `MMTK<VM>` instance that owns `lxr` and `self` (ensuring
    /// `worker.ordinal < self.gc_buffers.len()` and that no other `GCWorker` in this instance shares
    /// `worker.ordinal`), and no concurrent access to `self.gc_buffers[worker.ordinal]` or
    /// `self.local_packets[worker.ordinal]` (such as [`Self::flush_all`]) may occur during this call.
    pub(super) unsafe fn record(
        &self,
        worker: &mut crate::scheduler::GCWorker<VM>,
        s: VM::VMSlot,
        _o: ObjectReference,
        lxr: &LXR<VM>,
    ) {
        debug_assert!(std::ptr::eq(
            worker.mmtk.get_plan().downcast_ref::<LXR<VM>>().unwrap(),
            lxr
        ));
        debug_assert_eq!(
            crate::scheduler::current_worker_ordinal(),
            Some(worker.ordinal)
        );
        let id = worker.ordinal;
        let ix = lxr.immix_space.address_in_space(s.to_address());
        let entry = RemSetEntry::<VM>::encode(s, ix);
        let should_flush = {
            // SAFETY: By caller precondition, `worker` belongs to the `MMTK<VM>` instance owning
            // `self`, so `id < self.gc_buffers.len()`, and holding `&mut GCWorker<VM>` while no STW
            // `flush_all(&mut self)` is active guarantees exclusive access to `self.gc_buffers[id]`.
            // `buf` is dropped at the end of this block before `self.flush(id)` reborrows the cell.
            let buf = unsafe { &mut *self.gc_buffers[id].get() };
            buf.push(entry);
            buf.len() >= EvacuateMatureObjects::<VM>::CAPACITY
        };
        if should_flush {
            // SAFETY: `id < self.gc_buffers.len()`, `buf` above has ended its borrow, and
            // `&mut GCWorker<VM>` for this `MMTK<VM>` instance guarantees exclusive access to
            // `self.gc_buffers[id]` and `self.local_packets[id]`.
            unsafe { self.flush(id) };
        }
    }
}

#[derive(Default)]
pub struct MatureEvacuationSet {
    pub fragmented_blocks: SegQueue<Vec<(Block, usize)>>,
    pub fragmented_blocks_size: AtomicUsize,
    pub blocks_in_fragmented_chunks: SegQueue<Vec<(Block, usize)>>,
    pub blocks_in_fragmented_chunks_size: AtomicUsize,
    pub defrag_blocks: Mutex<Vec<Block>>,
    pub num_defrag_blocks: AtomicUsize,
}

impl MatureEvacuationSet {
    /// Release all the mature defrag source blocks
    pub fn sweep_mature_evac_candidates<VM: VMBinding>(&self, space: &ImmixSpace<VM>) {
        let mut defrag_blocks: Vec<Block> =
            std::mem::take(&mut *self.defrag_blocks.lock().unwrap());
        if defrag_blocks.is_empty() {
            return;
        }
        while let Some(block) = defrag_blocks.pop() {
            if !block.is_defrag_source() || block.get_state() == BlockState::Unallocated {
                // This block has been eagerly released (probably be reused again). Skip it.
                continue;
            }
            block.clear_rc_table();
            block.clear_striddle_table();
            block.rc_sweep_mature::<VM>(space, true);
            assert!(!block.is_defrag_source());
        }
    }

    pub fn schedule_defrag_selection_packets<VM: VMBinding>(&self, space: &ImmixSpace<VM>) {
        let tasks =
            space
                .chunk_map
                .generate_tasks_batched(space.scheduler().num_workers(), |chunks| {
                    Box::new(SelectDefragBlocks {
                        chunks,
                        defrag_threshold: 1,
                    })
                });
        self.fragmented_blocks_size.store(0, Ordering::SeqCst);
        SELECT_DEFRAG_BLOCK_JOB_COUNTER.store(tasks.len(), Ordering::SeqCst);
        space.scheduler().work_buckets[WorkBucketStage::Unconstrained].bulk_add(tasks);
    }

    pub fn skip_block(b: Block) -> bool {
        let s = b.get_state();
        b.is_defrag_source() || s == BlockState::Unallocated || s == BlockState::Nursery
    }

    fn select_fragmented_blocks(
        &self,
        selected_blocks: &mut Vec<Block>,
        copy_bytes: &mut usize,
        max_copy_bytes: usize,
    ) {
        let mut blocks = Vec::with_capacity(self.fragmented_blocks_size.load(Ordering::SeqCst));
        while let Some(mut x) = self.fragmented_blocks.pop() {
            blocks.append(&mut x);
        }
        blocks.sort_by_key(|x| x.1);
        while let Some((block, _dead_bytes)) = blocks.pop() {
            if Self::skip_block(block) {
                continue;
            }
            block.set_as_defrag_source(true);
            selected_blocks.push(block);
            *copy_bytes += (Block::BYTES - (block.calc_dead_lines() << Line::LOG_BYTES)) >> 1;
            if *copy_bytes >= max_copy_bytes {
                break;
            }
        }
    }

    #[allow(clippy::assertions_on_constants)]
    pub fn select_mature_evacuation_candidates<VM: VMBinding>(&self, lxr: &LXR<VM>) {
        debug_assert!(crate::plan::lxr::MATURE_EVACUATION);
        if lxr.current_pause().unwrap() == Pause::Full {
            // Make sure LOS sweeping finishes before evac selection begin
            // FIXME: This can be done in parallel with SelectDefragBlocksInChunk packets
            let los = lxr.common().get_los();
            los.release_rc_nursery_objects();
        }
        // Select mature defrag blocks
        let available_clean_pages_for_defrag = if lxr.current_pause().unwrap() == Pause::Full {
            lxr.get_total_pages()
                .saturating_sub(lxr.get_used_pages())
                .max(lxr.immix_space.defrag_headroom_pages())
        } else {
            lxr.immix_space.defrag_headroom_pages()
        };
        let max_copy_bytes = available_clean_pages_for_defrag << LOG_BYTES_IN_PAGE;
        let mut copy_bytes = 0usize;
        let mut selected_blocks = vec![];
        self.select_fragmented_blocks(&mut selected_blocks, &mut copy_bytes, max_copy_bytes);
        self.num_defrag_blocks
            .store(selected_blocks.len(), Ordering::SeqCst);
        let mut defrag_blocks = self.defrag_blocks.lock().unwrap();
        *defrag_blocks = selected_blocks;
        // cleanup
        assert!(self.fragmented_blocks.is_empty());
        assert!(self.blocks_in_fragmented_chunks.is_empty());
        self.fragmented_blocks_size.store(0, Ordering::SeqCst);
        self.blocks_in_fragmented_chunks_size
            .store(0, Ordering::SeqCst);
    }
}
