use std::collections::HashSet;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

use crate::{
    global_state::GcStatus,
    plan::tracing::{ObjectQueue, OptionObjectQueue},
    policy::{
        marksweepspace::native_ms::*,
        sft::{GCWorkerMutRef, SFT},
        space::{CommonSpace, Space},
    },
    scheduler::{GCWork, GCWorkScheduler, GCWorker, WorkBucketStage},
    util::{
        alloc::allocator::AllocationOptions,
        constants::LOG_BYTES_IN_PAGE,
        copy::CopySemantics,
        epilogue,
        heap::{
            chunk_map::{Chunk, ChunkMap},
            space_descriptor::SpaceDescriptor,
            BlockPageResource, PageResource,
        },
        linear_scan::{Region, UnstraddlableRegion},
        metadata::{self, side_metadata::SideMetadataSpec, MetadataSpec},
        object_enum::{self, ObjectEnumerator},
        ObjectReference, VMThread,
    },
    vm::{ActivePlan, ObjectModel, Scanning, VMBinding},
    MMTK,
};

#[cfg(feature = "vo_bit")]
use crate::util::Address;

use super::super::ChunkSweepLease;

/// The result for `MarkSweepSpace.acquire_block()`. `MarkSweepSpace` will attempt
/// to allocate from abandoned blocks first. If none found, it will get a new block
/// from the page resource.
pub enum BlockAcquireResult {
    Exhausted,
    /// A freshly initialized block we just acquired from the page resource.
    Fresh(OwnedBlock),
    /// An available swept block. The block can be directly used if there is any free cell in it.
    AbandonedAvailable(OwnedBlock),
    /// An unswept marked block. The block must be linearly swept first before it can be used.
    AbandonedUnswept(OwnedBlock),
}

/// A mark sweep space.
///
/// The space and each free list allocator own some block lists.
/// A block that is in use belongs to exactly one of the block lists. In this case,
/// whoever owns a block list has exclusive access on the blocks in the list.
/// There should be no data race to access blocks. A thread should NOT access a block list
/// if it does not own the block list.
pub struct MarkSweepSpace<VM: VMBinding> {
    pub common: CommonSpace<VM>,
    pr: BlockPageResource<VM, Block>,
    /// Allocation status for all chunks in MS space
    chunk_map: ChunkMap,
    /// Work packet scheduler
    scheduler: Arc<GCWorkScheduler<VM>>,
    /// Abandoned blocks. If a mutator dies, all its blocks go to this abandoned block
    /// lists. We reuse blocks in these lists in the mutator phase.
    abandoned: Mutex<AbandonedBlockLists>,
    /// Abandoned blocks during a GC. Filled in the release phase and merged into `abandoned`
    /// at the end of release.
    abandoned_in_gc: Mutex<AbandonedBlockLists>,
    /// Count the number of pending `ReleaseMarkSweepSpace` and `ReleaseMutator` work packets during
    /// the `Release` stage.
    pending_release_packets: AtomicUsize,
    /// True while STW `SweepChunk` work packets are sweeping blocks in place under `eager_sweeping`.
    eager_sweep_in_progress: AtomicBool,
    /// Per-chunk lease table preventing duplicate concurrent `SweepChunk` execution on the same chunk.
    sweeping_chunks: Mutex<HashSet<Chunk>>,
}

const fn _assert_marksweepspace_send_sync<VM: VMBinding>() {
    const fn check<T: Send + Sync>() {}
    check::<MarkSweepSpace<VM>>();
}

#[cfg(feature = "mock_test")]
static_assertions::assert_impl_all!(
    MarkSweepSpace<crate::util::test_util::mock_vm::MockVM>: Send,
    Sync
);

pub struct AbandonedBlockLists {
    available: BlockLists,
    unswept: BlockLists,
    consumed: BlockLists,
}

impl AbandonedBlockLists {
    fn new() -> Self {
        Self {
            available: new_empty_block_lists(),
            unswept: new_empty_block_lists(),
            consumed: new_empty_block_lists(),
        }
    }

    /// Move all blocks from a mutator's local block lists into `self`.
    pub(crate) fn abandon_from(
        &mut self,
        available: &mut BlockLists,
        available_stress: &mut BlockLists,
        consumed: &mut BlockLists,
        unswept: &mut BlockLists,
    ) {
        for i in 0..MI_BIN_FULL {
            let bin = BinIndex::new(i).unwrap();
            if !available[bin].is_empty() {
                self.available.append_bin_from(bin, available);
            }
            if !available_stress[bin].is_empty() {
                self.available.append_bin_from(bin, available_stress);
            }
            if !consumed[bin].is_empty() {
                self.consumed.append_bin_from(bin, consumed);
            }
            if !unswept[bin].is_empty() {
                self.unswept.append_bin_from(bin, unswept);
            }
        }
    }

    /// Release unmarked blocks in all bins back to `space`, and (under lazy sweeping) move
    /// surviving `BlockState::Marked` blocks into `self.unswept`.
    fn sweep_later<VM: VMBinding>(&mut self, space: &MarkSweepSpace<VM>) {
        for i in 0..MI_BIN_FULL {
            let bin = BinIndex::new(i).unwrap();
            if cfg!(feature = "eager_sweeping") {
                debug_assert!(self.unswept[bin].is_empty());
                self.available.release_blocks(bin, space);
                self.consumed.release_blocks(bin, space);
            } else {
                self.unswept.release_blocks(bin, space);
                self.available.release_blocks_and_move_marked_to_unswept(
                    bin,
                    space,
                    &mut self.unswept,
                );
                self.consumed.release_blocks_and_move_marked_to_unswept(
                    bin,
                    space,
                    &mut self.unswept,
                );
            }
        }
    }

    fn recycle_blocks(&mut self) {
        for i in 0..MI_BIN_FULL {
            let bin = BinIndex::new(i).unwrap();
            self.consumed
                .transfer_available_bin_to(bin, &mut self.available);
        }
    }

    fn merge(&mut self, other: &mut Self) {
        for i in 0..MI_BIN_FULL {
            let bin = BinIndex::new(i).unwrap();
            self.available.append_bin_from(bin, &mut other.available);
            self.unswept.append_bin_from(bin, &mut other.unswept);
            self.consumed.append_bin_from(bin, &mut other.consumed);
        }
    }

    #[cfg(debug_assertions)]
    fn assert_empty(&self) {
        for i in 0..MI_BIN_FULL {
            let bin = BinIndex::new(i).unwrap();
            assert!(self.available[bin].is_empty());
            assert!(self.unswept[bin].is_empty());
            assert!(self.consumed[bin].is_empty());
        }
    }
}

impl<VM: VMBinding> SFT for MarkSweepSpace<VM> {
    fn name(&self) -> &'static str {
        self.common.name
    }

    fn is_live(&self, object: ObjectReference) -> bool {
        VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.is_marked::<VM>(object, Ordering::SeqCst)
    }

    #[cfg(feature = "object_pinning")]
    fn pin_object(&self, _object: ObjectReference) -> bool {
        false
    }

    #[cfg(feature = "object_pinning")]
    fn unpin_object(&self, _object: ObjectReference) -> bool {
        false
    }

    #[cfg(feature = "object_pinning")]
    fn is_object_pinned(&self, _object: ObjectReference) -> bool {
        true
    }

    fn is_movable(&self) -> bool {
        false
    }

    #[cfg(feature = "sanity")]
    fn is_sane(&self) -> bool {
        true
    }

    fn initialize_object_metadata(&self, _object: ObjectReference, _bytes: usize) {
        #[cfg(feature = "vo_bit")]
        crate::util::metadata::vo_bit::set_vo_bit(_object);
    }

    #[cfg(feature = "vo_bit")]
    fn is_mmtk_object(&self, addr: Address) -> Option<ObjectReference> {
        crate::util::metadata::vo_bit::is_vo_bit_set_for_addr(addr)
    }

    #[cfg(feature = "vo_bit")]
    fn find_object_from_internal_pointer(
        &self,
        ptr: Address,
        max_search_bytes: usize,
    ) -> Option<ObjectReference> {
        let search_bytes = usize::min(MAX_OBJECT_SIZE, max_search_bytes);
        crate::util::metadata::vo_bit::find_object_from_internal_pointer::<VM>(ptr, search_bytes)
    }

    fn sft_trace_object(
        &self,
        queue: &mut OptionObjectQueue,
        object: ObjectReference,
        _worker: GCWorkerMutRef,
    ) -> ObjectReference {
        self.trace_object(queue, object)
    }
}

impl<VM: VMBinding> Space<VM> for MarkSweepSpace<VM> {
    fn as_space(&self) -> &dyn Space<VM> {
        self
    }

    fn as_sft(&self) -> &(dyn SFT + Sync + 'static) {
        self
    }

    fn get_page_resource(&self) -> &dyn PageResource<VM> {
        &self.pr
    }

    fn maybe_get_page_resource_mut(&mut self) -> Option<&mut dyn PageResource<VM>> {
        Some(&mut self.pr)
    }

    fn common(&self) -> &CommonSpace<VM> {
        &self.common
    }

    fn release_multiple_pages(&mut self, _start: crate::util::Address) {
        todo!()
    }

    fn enumerate_objects(&self, enumerator: &mut dyn ObjectEnumerator) {
        object_enum::enumerate_blocks_from_chunk_map::<Block>(enumerator, &self.chunk_map);
    }

    fn clear_side_log_bits(&self) {
        let log_bit = VM::VMObjectModel::GLOBAL_LOG_BIT_SPEC.extract_side_spec();
        for chunk in self.iter_allocated_chunks() {
            log_bit.bzero_metadata(chunk.start(), Chunk::BYTES);
        }
    }

    fn set_side_log_bits(&self) {
        let log_bit = VM::VMObjectModel::GLOBAL_LOG_BIT_SPEC.extract_side_spec();
        for chunk in self.iter_allocated_chunks() {
            log_bit.bset_metadata(chunk.start(), Chunk::BYTES);
        }
    }
}

impl<VM: VMBinding> crate::policy::gc_work::PolicyTraceObject<VM> for MarkSweepSpace<VM> {
    fn trace_object<Q: ObjectQueue, const KIND: crate::policy::gc_work::TraceKind>(
        &self,
        queue: &mut Q,
        object: ObjectReference,
        _copy: Option<CopySemantics>,
        _worker: &mut GCWorker<VM>,
    ) -> ObjectReference {
        self.trace_object(queue, object)
    }

    fn may_move_objects<const KIND: crate::policy::gc_work::TraceKind>() -> bool {
        false
    }
}

// We cannot allocate objects that are larger than the max bin size.
#[allow(dead_code)]
pub const MAX_OBJECT_SIZE: usize = crate::policy::marksweepspace::native_ms::MI_LARGE_OBJ_SIZE_MAX;

impl<VM: VMBinding> MarkSweepSpace<VM> {
    // Allow ptr_arg as we want to keep the function signature the same as for malloc marksweep
    #[allow(clippy::ptr_arg)]
    pub fn extend_global_side_metadata_specs(_specs: &mut Vec<SideMetadataSpec>) {}

    pub fn new(args: crate::policy::space::PlanCreateSpaceArgs<VM>) -> MarkSweepSpace<VM> {
        assert!(
            VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.is_on_side(),
            "MarkSweepSpace requires LOCAL_MARK_BIT_SPEC to be on the side"
        );
        let scheduler = args.scheduler.clone();
        let vm_map = args.vm_map;
        let is_discontiguous = args.vmrequest.is_discontiguous();
        let local_specs = metadata::extract_side_metadata(&[
            MetadataSpec::OnSide(Block::NEXT_BLOCK_TABLE),
            MetadataSpec::OnSide(Block::FREE_LIST_TABLE),
            MetadataSpec::OnSide(Block::SIZE_TABLE),
            #[cfg(feature = "malloc_native_mimalloc")]
            MetadataSpec::OnSide(Block::LOCAL_FREE_LIST_TABLE),
            #[cfg(feature = "malloc_native_mimalloc")]
            MetadataSpec::OnSide(Block::THREAD_FREE_LIST_TABLE),
            MetadataSpec::OnSide(Block::MARK_TABLE),
            *VM::VMObjectModel::LOCAL_MARK_BIT_SPEC,
        ]);
        let common = CommonSpace::new(args.into_policy_args(false, false, local_specs));
        let space_index = common.descriptor.get_index();
        MarkSweepSpace {
            pr: if is_discontiguous {
                BlockPageResource::new_discontiguous(
                    Block::LOG_PAGES,
                    vm_map,
                    scheduler.num_workers(),
                )
            } else {
                BlockPageResource::new_contiguous(
                    Block::LOG_PAGES,
                    common.start,
                    common.extent,
                    vm_map,
                    scheduler.num_workers(),
                )
            },
            common,
            chunk_map: ChunkMap::new(space_index),
            scheduler,
            abandoned: Mutex::new(AbandonedBlockLists::new()),
            abandoned_in_gc: Mutex::new(AbandonedBlockLists::new()),
            pending_release_packets: AtomicUsize::new(0),
            eager_sweep_in_progress: AtomicBool::new(false),
            sweeping_chunks: Mutex::new(HashSet::new()),
        }
    }

    fn attempt_mark_non_atomic(&self, object: ObjectReference) -> bool {
        if !VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.is_marked::<VM>(object, Ordering::SeqCst) {
            VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.mark::<VM>(object, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    fn attempt_mark_atomic(&self, object: ObjectReference) -> bool {
        let mark_state = 1u8;
        loop {
            let old_value = VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.load_atomic::<VM, u8>(
                object,
                None,
                Ordering::SeqCst,
            );
            if old_value == mark_state {
                return false;
            }
            if VM::VMObjectModel::LOCAL_MARK_BIT_SPEC
                .compare_exchange_metadata::<VM, u8>(
                    object,
                    old_value,
                    mark_state,
                    None,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
            {
                break;
            }
        }
        true
    }

    fn attempt_mark(&self, object: ObjectReference) -> bool {
        if VM::VMScanning::UNIQUE_OBJECT_ENQUEUING {
            self.attempt_mark_atomic(object)
        } else {
            self.attempt_mark_non_atomic(object)
        }
    }

    fn trace_object<Q: ObjectQueue>(
        &self,
        queue: &mut Q,
        object: ObjectReference,
    ) -> ObjectReference {
        debug_assert!(
            self.in_space(object),
            "Cannot mark an object {} that was not alloced by free list allocator.",
            object,
        );
        if self.attempt_mark(object) {
            let block = Block::containing(object);
            block.set_state(BlockState::Marked);
            queue.enqueue(object);
        }
        object
    }

    pub fn record_new_block(&self, block: Block) {
        assert!(
            self.address_in_space(block.start()),
            "MarkSweepSpace::record_new_block: block {:?} does not belong to space {}",
            block,
            self.common.name
        );
        block.init();
        self.chunk_map.set_allocated(block.chunk(), true);
    }

    /// Iterate all allocated chunks in `self.chunk_map` that genuinely belong to `self`
    /// (`self.address_in_space(chunk.start())`), filtering out any foreign chunks whose 4-bit
    /// `ChunkState` `space_index` collides with `self.common.descriptor.get_index()`.
    fn iter_allocated_chunks(&self) -> impl Iterator<Item = Chunk> + '_ {
        self.chunk_map
            .all_chunks()
            .filter(|chunk| self.address_in_space(chunk.start()))
    }

    pub fn prepare(&self, _full_heap: bool) {
        #[cfg(debug_assertions)]
        self.abandoned_in_gc.lock().unwrap().assert_empty();

        let space_descriptor = self.common.descriptor;
        let work_packets: Vec<Box<dyn GCWork<VM>>> = self
            .iter_allocated_chunks()
            .map(|chunk| -> Box<dyn GCWork<VM>> {
                Box::new(PrepareChunkMap {
                    space_descriptor,
                    chunk,
                })
            })
            .collect();
        self.scheduler.work_buckets[WorkBucketStage::Prepare].bulk_add(work_packets);
    }

    pub fn release(&self) {
        let num_mutators = VM::VMActivePlan::number_of_mutators();
        self.pending_release_packets
            .store(num_mutators + 1, Ordering::SeqCst);

        let work_packet = ReleaseMarkSweepSpace {
            space_descriptor: self.common.descriptor,
        };
        self.scheduler.work_buckets[WorkBucketStage::Release].add(work_packet);
    }

    pub fn end_of_gc(&self) {
        epilogue::debug_assert_counter_zero(
            &self.pending_release_packets,
            "pending_release_packets",
        );
    }

    /// Release a detached [`OwnedBlock`] back to the page resource after verifying space membership
    /// and zeroing its local metadata.
    pub(super) fn release_block(&self, block: OwnedBlock) {
        let raw = block.id();
        assert!(
            self.address_in_space(raw.start()),
            "MarkSweepSpace::release_block: block {:?} does not belong to space {}",
            raw,
            self.common.name
        );
        self.block_clear_metadata(raw);
        raw.deinit();
        self.pr.release_block(raw);
    }

    fn block_clear_metadata(&self, block: Block) {
        for metadata_spec in Block::METADATA_SPECS {
            metadata_spec.set_zero_atomic(block.start(), Ordering::SeqCst);
        }
        #[cfg(feature = "malloc_native_mimalloc")]
        {
            Block::LOCAL_FREE_LIST_TABLE.set_zero_atomic(block.start(), Ordering::SeqCst);
            Block::THREAD_FREE_LIST_TABLE.set_zero_atomic(block.start(), Ordering::SeqCst);
        }
        #[cfg(feature = "vo_bit")]
        crate::util::metadata::vo_bit::bzero_vo_bit(block.start(), Block::BYTES);
    }

    pub fn acquire_block(
        &self,
        tls: VMThread,
        size: usize,
        align: usize,
        alloc_options: AllocationOptions,
    ) -> BlockAcquireResult {
        assert!(
            !self.eager_sweep_in_progress.load(Ordering::Acquire),
            "MarkSweepSpace::acquire_block called while STW eager sweeping is in progress"
        );
        let bin = mi_bin_bounded::<VM>(size, align);
        {
            let mut abandoned = self.abandoned.lock().unwrap();

            if let Some(block) = abandoned.available.pop(bin) {
                assert!(self.address_in_space(block.start()));
                return BlockAcquireResult::AbandonedAvailable(block);
            }

            if let Some(block) = abandoned.unswept.pop(bin) {
                assert!(self.address_in_space(block.start()));
                return BlockAcquireResult::AbandonedUnswept(block);
            }
        }

        let acquired = self.acquire(tls, Block::BYTES >> LOG_BYTES_IN_PAGE, alloc_options);
        if acquired.is_zero() {
            BlockAcquireResult::Exhausted
        } else {
            let raw_block = Block::from_aligned_address(acquired);
            self.record_new_block(raw_block);
            // SAFETY: `self.acquire` returned a freshly acquired, 64 KiB-aligned block in `self`
            // with mapped heap pages `[raw_block.start(), raw_block.end())` and mapped side
            // metadata, `record_new_block` initialized `MARK_TABLE = BlockState::Unmarked` and
            // `ChunkMap`, no other `OwnedBlock` or `BlockList` holds `raw_block`, and
            // `init_free_cells(bin)` immediately establishes the size-class free-cell chain.
            let mut block = unsafe { OwnedBlock::from_block_unchecked(raw_block) };
            block.init_free_cells(bin);
            BlockAcquireResult::Fresh(block)
        }
    }

    /// Transfer all local block lists of a destroyed mutator into `self.abandoned`.
    pub(crate) fn abandon_blocks(
        &self,
        available: &mut BlockLists,
        available_stress: &mut BlockLists,
        consumed: &mut BlockLists,
        unswept: &mut BlockLists,
    ) {
        assert!(
            !self.eager_sweep_in_progress.load(Ordering::Acquire),
            "MarkSweepSpace::abandon_blocks called while STW eager sweeping is in progress"
        );
        self.abandoned
            .lock()
            .unwrap()
            .abandon_from(available, available_stress, consumed, unswept);
    }

    /// Transfer all local block lists of a mutator during `Release` into `self.abandoned_in_gc`.
    pub(crate) fn abandon_blocks_in_gc(
        &self,
        available: &mut BlockLists,
        available_stress: &mut BlockLists,
        consumed: &mut BlockLists,
        unswept: &mut BlockLists,
    ) {
        assert!(
            !self.eager_sweep_in_progress.load(Ordering::Acquire),
            "MarkSweepSpace::abandon_blocks_in_gc called while STW eager sweeping is in progress"
        );
        self.abandoned_in_gc.lock().unwrap().abandon_from(
            available,
            available_stress,
            consumed,
            unswept,
        );
    }

    pub fn release_packet_done(&self) {
        let old = self.pending_release_packets.fetch_sub(1, Ordering::SeqCst);
        if old == 1 {
            if cfg!(feature = "eager_sweeping") {
                let work_packets = self.generate_sweep_tasks();
                if work_packets.is_empty() {
                    self.recycle_blocks();
                } else {
                    self.eager_sweep_in_progress.store(true, Ordering::Release);
                    self.scheduler.work_buckets[WorkBucketStage::Release].bulk_add(work_packets);
                }
            } else {
                self.recycle_blocks();
            }
        }
    }

    fn generate_sweep_tasks(&self) -> Vec<Box<dyn GCWork<VM>>> {
        let space_descriptor = self.common.descriptor;
        let epilogue = Arc::new(SweepChunkEpilogue {
            counter: AtomicUsize::new(0),
        });
        let tasks: Vec<Box<dyn GCWork<VM>>> = self
            .iter_allocated_chunks()
            .map(|chunk| -> Box<dyn GCWork<VM>> {
                Box::new(SweepChunk {
                    space_descriptor,
                    chunk,
                    epilogue: epilogue.clone(),
                })
            })
            .collect();
        epilogue.counter.store(tasks.len(), Ordering::SeqCst);
        tasks
    }

    fn sweep_allocated_chunk(&self, mmtk: &'static MMTK<VM>, chunk: Chunk) {
        assert_eq!(
            mmtk.state.gc_status.load(),
            GcStatus::InPause,
            "MarkSweepSpace::sweep_allocated_chunk requires an active STW GC pause"
        );
        assert!(
            self.eager_sweep_in_progress.load(Ordering::Acquire),
            "MarkSweepSpace::sweep_allocated_chunk called outside STW eager sweep phase"
        );
        assert_eq!(
            self.pending_release_packets.load(Ordering::Acquire),
            0,
            "MarkSweepSpace::sweep_allocated_chunk requires all release packets to finish first"
        );
        let _lease = ChunkSweepLease::acquire(&self.sweeping_chunks, chunk, "MarkSweepSpace");
        assert!(
            self.chunk_map.is_allocated_checked(chunk) && self.chunk_map.get(chunk).is_some(),
            "MarkSweepSpace::sweep_allocated_chunk: chunk {:?} is not allocated in space {}",
            chunk,
            self.common.name
        );
        assert!(self.address_in_space(chunk.start()));

        let mut allocated_blocks = 0;
        for block in chunk
            .iter_region::<Block>()
            .filter(|block| block.get_state() != BlockState::Unallocated)
        {
            // SAFETY: All mutators are parked in `GcStatus::InPause`, all `ReleaseMutator` and
            // `ReleaseMarkSweepSpace` packets completed (`pending_release_packets == 0`),
            // `eager_sweep_in_progress` prevents any concurrent `abandoned` / `abandoned_in_gc`
            // access, and `_lease` guarantees exclusive per-chunk sweeping of `block` in `self`.
            unsafe { block.sweep_cells::<VM>() };
            allocated_blocks += 1;
        }
        probe!(mmtk, sweep_chunk_ms, allocated_blocks);
        if allocated_blocks == 0 {
            self.chunk_map.set_allocated(chunk, false);
        }
    }

    fn recycle_blocks(&self) {
        {
            let mut abandoned = self.abandoned.try_lock().unwrap();
            let mut abandoned_in_gc = self.abandoned_in_gc.try_lock().unwrap();

            if cfg!(feature = "eager_sweeping") {
                abandoned.recycle_blocks();
                abandoned_in_gc.recycle_blocks();
            }

            abandoned.merge(&mut abandoned_in_gc);

            #[cfg(debug_assertions)]
            abandoned_in_gc.assert_empty();
        }

        self.pr.flush_all();
        self.eager_sweep_in_progress.store(false, Ordering::Release);
    }
}

fn with_ms_space<VM: VMBinding, R>(
    mmtk: &'static MMTK<VM>,
    descriptor: SpaceDescriptor,
    f: impl FnOnce(&MarkSweepSpace<VM>) -> R,
) -> R {
    let mut f = Some(f);
    let mut out = None;
    mmtk.get_plan().for_each_space(&mut |s| {
        if let Some(ms) = s.downcast_ref::<MarkSweepSpace<VM>>() {
            if ms.common.descriptor == descriptor {
                out = Some((f.take().unwrap())(ms));
            }
        }
    });
    out.expect("MarkSweepSpace must be registered in Plan::for_each_space")
}

struct PrepareChunkMap {
    space_descriptor: SpaceDescriptor,
    chunk: Chunk,
}

impl<VM: VMBinding> GCWork<VM> for PrepareChunkMap {
    fn do_work(&mut self, _worker: &mut GCWorker<VM>, mmtk: &'static MMTK<VM>) {
        with_ms_space(mmtk, self.space_descriptor, |space| {
            debug_assert!(space.chunk_map.get(self.chunk).unwrap().is_allocated());
            let mut n_occupied_blocks = 0;
            self.chunk
                .iter_region::<Block>()
                .filter(|block| block.get_state() != BlockState::Unallocated)
                .for_each(|block| {
                    block.set_state(BlockState::Unmarked);
                    n_occupied_blocks += 1;
                });
            if n_occupied_blocks == 0 {
                space.chunk_map.set_allocated(self.chunk, false);
            } else if let MetadataSpec::OnSide(side) = *VM::VMObjectModel::LOCAL_MARK_BIT_SPEC {
                side.bzero_metadata(self.chunk.start(), Chunk::BYTES);
            }
        });
    }
}

struct ReleaseMarkSweepSpace {
    space_descriptor: SpaceDescriptor,
}

impl<VM: VMBinding> GCWork<VM> for ReleaseMarkSweepSpace {
    fn do_work(&mut self, _worker: &mut GCWorker<VM>, mmtk: &'static MMTK<VM>) {
        with_ms_space(mmtk, self.space_descriptor, |space| {
            {
                let mut abandoned = space.abandoned.lock().unwrap();
                abandoned.sweep_later(space);
            }
            space.release_packet_done();
        });
    }
}

struct SweepChunk {
    space_descriptor: SpaceDescriptor,
    chunk: Chunk,
    epilogue: Arc<SweepChunkEpilogue>,
}

impl<VM: VMBinding> GCWork<VM> for SweepChunk {
    fn do_work(&mut self, _worker: &mut GCWorker<VM>, mmtk: &'static MMTK<VM>) {
        with_ms_space(mmtk, self.space_descriptor, |space| {
            space.sweep_allocated_chunk(mmtk, self.chunk);
            if 1 == self.epilogue.counter.fetch_sub(1, Ordering::SeqCst) {
                space.recycle_blocks();
            }
        });
    }
}

struct SweepChunkEpilogue {
    counter: AtomicUsize,
}

impl Drop for SweepChunkEpilogue {
    fn drop(&mut self) {
        epilogue::debug_assert_counter_zero(&self.counter, "SweepChunkEpilogue::counter");
    }
}
