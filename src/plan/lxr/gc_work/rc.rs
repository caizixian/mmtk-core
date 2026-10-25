use super::super::LazySweepingJobsCounter;
use super::super::SurvivalRatioPredictorLocal;
use super::super::LXR;
use super::super::{LAZY_DECREMENTS, MATURE_EVACUATION, NO_EVAC, NURSERY_EVACUATION};
use super::tracing::LXRConcurrentTraceObjects;
use super::tracing::LXRStopTheWorldProcessEdges;
#[cfg(feature = "non_moving_root")]
use super::tracing::LXRStopTheWorldProcessNodes;
#[cfg(feature = "non_moving_root")]
use super::tracing::ProcessModBufSATB;
use super::ProcessEdgesBase;
use crate::plan::VectorQueue;
use crate::policy::immix::block::BlockState;
use crate::scheduler::gc_work::RootKind;
use crate::util::copy::CopySemantics;
use crate::util::linear_scan::UnstraddlableRegion;
use crate::util::metadata::side_metadata::SideMetadataSpec;
use crate::util::rc::*;
use crate::vm::slot::Slot;
use crate::{
    plan::concurrent::global::ConcurrentPlan,
    plan::concurrent::Pause,
    plan::global::Plan,
    policy::{immix::block::Block, space::Space},
    scheduler::{GCWork, GCWorker, WorkBucketStage},
    util::{object_forwarding, ObjectReference},
    vm::*,
    MMTK,
};
use atomic::Ordering;
#[cfg(feature = "non_moving_root")]
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

pub struct ProcessIncs<VM: VMBinding, const KIND: EdgeKind> {
    /// Increments to process
    incs: Vec<VM::VMSlot>,
    /// Recursively generated new increments
    new_incs: VectorQueue<VM::VMSlot>,
    new_incs_count: u32,
    pause: Pause,
    in_cm: bool,
    no_evac: bool,
    pub root_kind: Option<RootKind>,
    depth: u32,
    lxr: &'static LXR<VM>,
    rc: RefCountHelper<VM>,
    survival_ratio_predictor_local: SurvivalRatioPredictorLocal,
}

impl<VM: VMBinding, const KIND: EdgeKind> ProcessIncs<VM, KIND> {
    const CAPACITY: usize = 1024;
    const UNLOG_BITS: SideMetadataSpec = *VM::VMObjectModel::GLOBAL_FIELD_UNLOG_BIT_SPEC
        .as_spec()
        .extract_side_spec();

    fn __default(lxr: &'static LXR<VM>) -> Self {
        Self {
            incs: vec![],
            new_incs: VectorQueue::default(),
            new_incs_count: 0,
            lxr,
            pause: Pause::RefCount,
            in_cm: false,
            no_evac: false,
            depth: 1,
            rc: RefCountHelper::NEW,
            root_kind: None,
            survival_ratio_predictor_local: SurvivalRatioPredictorLocal::default(),
        }
    }

    fn add_new_slot(&mut self, worker: &mut GCWorker<VM>, s: VM::VMSlot) {
        self.new_incs.push(s);
        self.new_incs_count += 1;
        if self.new_incs_count as usize >= Self::CAPACITY {
            self.flush(worker);
        }
    }

    pub fn new(incs: Vec<VM::VMSlot>, lxr: &'static LXR<VM>) -> Self {
        Self {
            incs,
            ..Self::__default(lxr)
        }
    }

    #[cfg(feature = "non_moving_root")]
    /// Reference-count root objects reported as objects rather than as slots.
    /// Node counterpart of `process_incs::<EDGE_KIND_ROOT>`. Nothing here may move an object.
    fn process_root_nodes(
        &mut self,
        worker: &mut GCWorker<VM>,
        nodes: Vec<ObjectReference>,
    ) -> (Vec<ObjectReference>, Vec<ObjectReference>) {
        let mut roots = Vec::with_capacity(nodes.len());
        let mut uncounted = vec![];
        for o in nodes {
            if !self.lxr.is_rc_object(o) {
                uncounted.push(o);
                continue;
            }
            let los = self.lxr.los().in_space(o);
            if self.inc(o) {
                // Promote without moving.
                self.promote(worker, o, false, los, 0);
            }
            roots.push(o);
        }
        // Promotion above can queue further increments; hand them on rather than
        // dropping them.
        self.flush(worker);
        (roots, uncounted)
    }

    #[cfg(feature = "non_moving_root")]
    /// Node-shaped counterpart of [`GCWork::do_work`] below for `KIND == EDGE_KIND_ROOT`.
    /// However, the major difference is that we cannot move any of these objects here, as
    /// we don't know their slots.
    pub fn do_work_root_nodes(
        &mut self,
        nodes: Vec<ObjectReference>,
        worker: &mut GCWorker<VM>,
        mmtk: &'static MMTK<VM>,
    ) {
        debug_assert_eq!(KIND, EDGE_KIND_ROOT);
        let lxr = self.lxr;
        self.pause = lxr.current_pause().unwrap();
        self.in_cm = lxr.concurrent_work_in_progress();
        let (roots, uncounted) = self.process_root_nodes(worker, nodes);
        let mut to_trace = roots.clone();
        to_trace.extend_from_slice(&uncounted);
        if to_trace.is_empty() {
            return;
        }
        // Roots arriving mid-cycle must also enter the SATB snapshot, or the cycle
        // collector can conclude their subgraphs are unreachable.
        if lxr.cm_enabled() && self.in_cm {
            worker.add_work(
                WorkBucketStage::FinishConcurrentWork,
                ProcessModBufSATB::new(to_trace.clone()),
            );
        }
        // Nodes, unlike slots, are never re-scanned, so a stop-the-world tracing pause has to
        // mark from them here or they are never marked at all.
        match self.pause {
            Pause::Full => worker.add_work(
                WorkBucketStage::PinningRootsTrace,
                LXRStopTheWorldProcessNodes::<VM, true>::new(
                    to_trace,
                    mmtk,
                    WorkBucketStage::Closure,
                ),
            ),
            Pause::FinalMark => worker.add_work(
                WorkBucketStage::PinningRootsTrace,
                LXRStopTheWorldProcessNodes::<VM, false>::new(
                    to_trace,
                    mmtk,
                    WorkBucketStage::Closure,
                ),
            ),
            // Seed concurrent marking with these roots, exactly as the slot path does --
            // otherwise node roots seed nothing until `FinalMark`.
            Pause::InitialMark if lxr.cm_enabled() => {
                worker.scheduler().work_buckets[WorkBucketStage::ConcurrentResumable]
                    .add(LXRConcurrentTraceObjects::new(to_trace, mmtk));
            }
            // `RefCount` has no marking phase.
            _ => {}
        }
        // Recorded so the matching decrements are applied in the next pause.
        if !roots.is_empty() {
            lxr.curr_roots.read().unwrap().push(roots);
        }
    }

    fn promote(
        &mut self,
        worker: &mut GCWorker<VM>,
        o: ObjectReference,
        copied: bool,
        los: bool,
        depth: u32,
    ) {
        let size = o.get_size::<VM>();

        if !los {
            let block = Block::containing(o);
            let in_nursery_block = block.get_state() == BlockState::Nursery;
            if !copied && in_nursery_block {
                block.set_as_in_place_promoted();
            }
            self.rc.promote_with_size(o, size);
            self.survival_ratio_predictor_local
                .record_promotion(size, copied);
        } else {
            // println!("promote los {:?} {}", o, self.immix().is_marked(o));
        }
        // Don't mark copied objects in initial mark pause. The concurrent marker will do it (and can also resursively mark the old objects).
        if self.in_cm || self.pause == Pause::FinalMark {
            debug_assert!(self.lxr.is_marked(o), "{:?} is not marked", o);
        }
        self.scan_nursery_object(worker, o, los, !copied, depth, size);
    }

    fn record_mature_evac_remset2(
        &mut self,
        worker: &mut GCWorker<VM>,
        slot_in_defrag: bool,
        s: VM::VMSlot,
        o: ObjectReference,
    ) {
        if !(MATURE_EVACUATION && (self.in_cm || self.pause == Pause::FinalMark)) {
            return;
        }
        if !slot_in_defrag && self.lxr.in_defrag(o) {
            self.lxr.mature_evac_remset.record(worker, s, o, self.lxr);
        }
    }

    fn record_mature_evac_remset(
        &mut self,
        worker: &mut GCWorker<VM>,
        s: VM::VMSlot,
        o: ObjectReference,
    ) {
        if !(MATURE_EVACUATION && (self.in_cm || self.pause == Pause::FinalMark)) {
            return;
        }
        self.record_mature_evac_remset2(worker, self.lxr.address_in_defrag(s.to_address()), s, o);
    }

    fn scan_nursery_object(
        &mut self,
        worker: &mut GCWorker<VM>,
        o: ObjectReference,
        los: bool,
        in_place_promotion: bool,
        _depth: u32,
        size: usize,
    ) {
        if los {
            let obj_start = VM::VMObjectModel::ref_to_object_start(o);
            Self::UNLOG_BITS.bset_metadata(obj_start, size);
        } else if in_place_promotion {
            let start = VM::VMObjectModel::ref_to_object_start(o);
            #[cfg(feature = "extreme_assertions")]
            {
                let step = 1usize << Self::UNLOG_BITS.log_bytes_in_region;
                let end = (start + size).align_up(step);
                let mut cursor = start.align_down(step);
                while cursor < end {
                    Self::UNLOG_BITS.store_atomic::<u8>(
                        cursor,
                        crate::util::metadata::log_bit::UNLOGGED_VALUE,
                        Ordering::Relaxed,
                    );
                    cursor += step;
                }
            }
            #[cfg(not(feature = "extreme_assertions"))]
            {
                // Set every unlog bit covering `[start, start + size)` via 1-byte relaxed atomics
                // (`AtomicU8`). We align to the nearest `UNLOG_BITS` metadata byte, which may
                // benignly overset unlog bits for adjacent slots in the same 64-byte window.
                let heap_bytes_per_unlog_byte =
                    VM::VMObjectModel::GLOBAL_FIELD_UNLOG_BIT_SPEC.heap_bytes_per_unlog_byte();
                let mut cursor = start.align_down(heap_bytes_per_unlog_byte);
                let aligned_end = (start + size).align_up(heap_bytes_per_unlog_byte);
                while cursor < aligned_end {
                    // SAFETY: `Self::UNLOG_BITS` has `log_num_of_bits == 0 < 3`, and `o` resides in
                    // a single mapped Immix block (`Block::BYTES >= 32 KiB`, a multiple of
                    // `heap_bytes_per_unlog_byte`), so every side-metadata byte covering
                    // `[cursor, aligned_end)` is non-null, mapped, and initialized. Mutators are
                    // stopped during `ProcessIncs`, all concurrent `ProcessIncs` updates to Immix
                    // `UNLOG_BITS` use `AtomicU8` (`set_raw_byte_atomic`), and over-setting
                    // adjacent slot bits in the same byte to `1` (`UNLOGGED_VALUE`) is benign.
                    unsafe {
                        Self::UNLOG_BITS.set_raw_byte_atomic(cursor, Ordering::Relaxed);
                    }
                    cursor += heap_bytes_per_unlog_byte;
                }
            }
        }
        // Promotion above arms the per-field unlog bits. Do the same for object log bits.
        #[cfg(feature = "lxr_object_log")]
        VM::VMObjectModel::GLOBAL_LOG_BIT_SPEC.mark_as_unlogged::<VM>(o, Ordering::SeqCst);
        let obj_in_defrag = !los && Block::in_defrag_block(o);
        let tls = worker.tls.0;
        o.iterate_fields::<VM, _>(tls, |slot| {
            let Some(target) = slot.load() else {
                return;
            };
            debug_assert!(
                target.to_raw_address().is_mapped(),
                "Unmapped obj {:?}.{:?} -> {:?}",
                o,
                slot,
                target
            );
            debug_assert!(
                target.is_in_any_space(),
                "Unmapped obj {:?}.{:?} -> {:?}",
                o,
                slot,
                target
            );
            let rc = self.rc.count(target);
            if rc == 0 {
                self.add_new_slot(worker, slot);
            } else {
                if rc != crate::util::rc::MAX_REF_COUNT {
                    let _ = self.rc.inc(target);
                }
                self.record_mature_evac_remset2(worker, obj_in_defrag, slot, target);
            }
        });
    }

    #[cold]
    fn flush(&mut self, worker: &mut GCWorker<VM>) {
        if !self.new_incs.is_empty() {
            let new_incs = self.new_incs.take();
            let mut w = ProcessIncs::<VM, EDGE_KIND_NURSERY>::new(new_incs, self.lxr);
            w.depth += 1;
            worker.add_work(WorkBucketStage::Unconstrained, w);
        }
        self.new_incs_count = 0;
    }

    /// Return true if the object's ref count is incremented and the count was zero before the increment
    fn inc(&self, o: ObjectReference) -> bool {
        self.rc.inc(o) == Ok(0)
    }

    fn dont_evacuate(&self, o: ObjectReference, los: bool) -> bool {
        if los {
            return true;
        }
        // Skip mature object
        if self.rc.count(o) != 0 {
            return true;
        }
        // Skip recycled lines
        if Block::containing(o).get_state() != BlockState::Nursery {
            return true;
        }
        debug_assert!(o.to_raw_address().is_mapped());
        #[cfg(feature = "vo_bit")]
        debug_assert!(
            crate::util::metadata::vo_bit::is_vo_bit_set(o),
            "ERROR {:?} rc={}: VO bit not set",
            o,
            self.rc.count(o)
        );
        false
    }

    fn process_inc_and_evacuate(
        &mut self,
        worker: &mut GCWorker<VM>,
        o: ObjectReference,
        depth: u32,
    ) -> ObjectReference {
        let los = self.lxr.los().in_space(o);
        if NURSERY_EVACUATION && !los && object_forwarding::is_forwarded_or_being_forwarded::<VM>(o)
        {
            while object_forwarding::is_being_forwarded::<VM>(o) {
                std::hint::spin_loop();
            }
            let new = if object_forwarding::is_forwarded::<VM>(o) {
                object_forwarding::read_forwarding_pointer::<VM>(o)
            } else {
                o
            };
            let promoted = self.inc(new);
            if promoted && new == o {
                self.promote(worker, o, false, los, depth);
            }
            return new;
        }
        if !NURSERY_EVACUATION || self.dont_evacuate(o, los) {
            if self.inc(o) {
                self.promote(worker, o, false, los, depth);
            }
            return o;
        }
        let forwarding_status = object_forwarding::attempt_to_forward::<VM>(o);
        if object_forwarding::state_is_forwarded_or_being_forwarded(forwarding_status) {
            // Object is moved to a new location.
            let new = object_forwarding::spin_and_get_forwarded_object::<VM>(o, forwarding_status);
            self.inc(new);
            new
        } else {
            let is_nursery = self.rc.count(o) == 0;
            if is_nursery && !self.no_evac {
                // Evacuate the object
                let new = object_forwarding::try_forward_object::<VM>(
                    o,
                    CopySemantics::DefaultCopy,
                    worker.get_copy_context_mut(),
                    |_new| {
                        #[cfg(feature = "vo_bit")]
                        {
                            // Set the VO bit of the new object.
                            crate::util::metadata::vo_bit::set_vo_bit(_new);
                            // Clear the VO bit of the old object.
                            // Note that sweeping can also clear the VO bit when the line is freed,
                            // but no RC inc/dec should be performed on the old object from now on.
                            // We clear it eagerly to detect inc/dec errors.
                            crate::util::metadata::vo_bit::unset_vo_bit(o);
                        }
                    },
                );
                if let Some(new) = new {
                    self.inc(new);
                    self.promote(worker, new, true, false, depth);
                    new
                } else {
                    warn!("to-space overflow");
                    // Object is not moved.
                    let promoted = self.inc(o);
                    object_forwarding::clear_forwarding_bits::<VM>(o);
                    if promoted {
                        self.promote(worker, o, false, los, depth);
                    }
                    NO_EVAC.store(true, Ordering::Relaxed);
                    self.no_evac = true;
                    o
                }
            } else {
                // Object is not moved.
                let promoted = self.inc(o);
                object_forwarding::clear_forwarding_bits::<VM>(o);
                if promoted {
                    self.promote(worker, o, false, los, depth);
                }
                o
            }
        }
    }

    /// Return `None` if the increment of the slot should be delayed
    fn unlog_and_load_rc_object<const K: EdgeKind>(
        &mut self,
        s: VM::VMSlot,
    ) -> Option<ObjectReference> {
        let o = s.load();
        // unlog slot
        if K == EDGE_KIND_MATURE {
            // SAFETY: `Self::UNLOG_BITS` is a 1-bit (`log_num_of_bits == 0 < 3`) per-field side
            // metadata spec, `s` is a valid heap slot whose side metadata is mapped, and
            // over-setting adjacent bits in the same 64-byte (`heap_bytes_per_unlog_byte`) window
            // to `1` (`UNLOGGED_VALUE`) is benign (it only forces at most one slow-path barrier on
            // the next write to an adjacent slot) while callers never rely on adjacent bits being
            // set. `set_raw_byte_atomic` uses `AtomicU8` (`Ordering::Relaxed`), avoiding data races
            // and mixed-size atomic accesses against concurrent `ProcessIncs` workers.
            unsafe {
                Self::UNLOG_BITS.set_raw_byte_atomic(s.to_address(), Ordering::Relaxed);
            }
        }
        o
    }

    fn process_slot<const K: EdgeKind>(
        &mut self,
        worker: &mut GCWorker<VM>,
        s: VM::VMSlot,
        depth: u32,
        add_root_to_remset: bool,
    ) -> Option<ObjectReference> {
        let o = match self.unlog_and_load_rc_object::<K>(s) {
            Some(o) => o,
            _ => {
                return None;
            }
        };
        // println!(" - inc {:?}: {:?} rc={}", s, o, self.rc.count(o));
        let new = self.process_inc_and_evacuate(worker, o, depth);
        // Put this into remset if this is a mature slot, or a weak root
        if K != EDGE_KIND_ROOT || add_root_to_remset {
            self.record_mature_evac_remset(worker, s, new);
        }
        if new != o {
            s.store(new)
        }
        Some(new)
    }

    fn process_owned_incs<const K: EdgeKind>(
        &mut self,
        worker: &mut GCWorker<VM>,
        incs: Vec<VM::VMSlot>,
        depth: u32,
        add_root_to_remset: bool,
    ) -> Option<Vec<ObjectReference>> {
        if K == EDGE_KIND_ROOT {
            // `alloc::vec::in_place_collect` reuses `incs`'s heap buffer in place with zero
            // allocations whenever `VM::VMSlot` and `ObjectReference` have identical size and
            // alignment, and falls back to allocating a fresh `Vec` otherwise.
            let roots: Vec<ObjectReference> = incs
                .into_iter()
                .filter_map(|s| self.process_slot::<K>(worker, s, depth, add_root_to_remset))
                .collect();
            if roots.is_empty() {
                None
            } else {
                Some(roots)
            }
        } else {
            self.process_slice_incs::<K>(worker, &incs, depth);
            None
        }
    }

    fn process_slice_incs<const K: EdgeKind>(
        &mut self,
        worker: &mut GCWorker<VM>,
        incs: &[VM::VMSlot],
        depth: u32,
    ) {
        debug_assert_ne!(K, EDGE_KIND_ROOT);
        for s in incs {
            self.process_slot::<K>(worker, *s, depth, false);
        }
    }
}

pub type EdgeKind = u8;
pub const EDGE_KIND_ROOT: u8 = 0;
pub const EDGE_KIND_NURSERY: u8 = 1;
pub const EDGE_KIND_MATURE: u8 = 2;

impl<VM: VMBinding, const KIND: EdgeKind> GCWork<VM> for ProcessIncs<VM, KIND> {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, mmtk: &'static MMTK<VM>) {
        self.lxr = mmtk.get_plan().downcast_ref::<LXR<VM>>().unwrap();
        self.pause = self.lxr.current_pause().unwrap();
        self.in_cm = self.lxr.concurrent_work_in_progress();
        if NO_EVAC.load(Ordering::Relaxed) {
            self.no_evac = true;
        } else {
            let over_space = mmtk.get_plan().get_used_pages()
                - mmtk.get_plan().get_collection_reserved_pages()
                > mmtk.get_plan().get_total_pages();
            if over_space {
                self.no_evac = true;
                NO_EVAC.store(true, Ordering::Relaxed);
            }
        }
        // Process main buffer
        let root_slots = if KIND == EDGE_KIND_ROOT
            && (self.pause == Pause::FinalMark || self.pause == Pause::Full)
        {
            self.incs.clone()
        } else {
            vec![]
        };
        let roots = {
            let incs = std::mem::take(&mut self.incs);
            self.process_owned_incs::<KIND>(worker, incs, self.depth, false)
        };
        if let Some(roots) = roots {
            if self.lxr.cm_enabled()
                && self.pause == Pause::InitialMark
                && !self.root_kind.unwrap().should_skip_mark_and_decs()
            {
                if cfg!(any(feature = "sanity", debug_assertions)) {
                    for r in &roots {
                        assert!(
                            r.to_raw_address().is_mapped(),
                            "Invalid object {:?}: address is not mapped",
                            r
                        );
                    }
                }
                worker.scheduler().work_buckets[WorkBucketStage::ConcurrentResumable]
                    .add(LXRConcurrentTraceObjects::new(roots.clone(), mmtk));
            }
            if self.pause == Pause::FinalMark || self.pause == Pause::Full {
                if !root_slots.is_empty() && self.root_kind != Some(RootKind::Weak) {
                    if self.pause == Pause::FinalMark {
                        let mut w = LXRStopTheWorldProcessEdges::<_, false>::new(
                            root_slots,
                            true,
                            mmtk,
                            WorkBucketStage::Closure,
                        );
                        w.root_kind = self.root_kind;
                        worker.add_work(WorkBucketStage::Closure, w)
                    } else {
                        let mut w = LXRStopTheWorldProcessEdges::<_, true>::new(
                            root_slots,
                            true,
                            mmtk,
                            WorkBucketStage::Closure,
                        );
                        w.root_kind = self.root_kind;
                        worker.add_work(WorkBucketStage::Closure, w)
                    };
                }
            } else if !self.root_kind.unwrap().should_skip_decs() {
                self.lxr.curr_roots.read().unwrap().push(roots);
            }
        }
        // Process recursively generated buffer
        let mut depth = self.depth;
        let mut incs = vec![];
        // Incs are processed by the worker that discovered them, unless we split.
        // Split size defaults to `usize::MAX` (never split).
        let split_size = *self.lxr.base().options.lxr_min_packet_split_size;
        // Split depth defaults to 16 (split after 16 recursions).
        let split_depth = *self.lxr.base().options.lxr_min_packet_split_depth;
        while !self.new_incs.is_empty() {
            self.new_incs_count = 0;
            depth += 1;
            incs.clear();
            self.new_incs.swap(&mut incs);
            if depth as usize >= split_depth && incs.len() > split_size {
                let (a, b) = incs.split_at(incs.len() / 2);
                let mut w = ProcessIncs::<VM, EDGE_KIND_NURSERY>::new(b.to_vec(), self.lxr);
                w.depth = depth;
                worker.add_work(WorkBucketStage::Unconstrained, w);
                incs = a.to_vec();
            }
            if !incs.is_empty() {
                self.process_slice_incs::<EDGE_KIND_NURSERY>(worker, &incs, depth);
            }
        }
        self.survival_ratio_predictor_local.sync();
    }
}

pub struct ProcessDecs<VM: VMBinding> {
    /// Decrements to process
    decs: Option<Vec<ObjectReference>>,
    decs_arc: Option<Arc<Vec<ObjectReference>>>,
    /// Recursively generated new decrements
    new_decs: VectorQueue<ObjectReference>,
    counter: LazySweepingJobsCounter,
    mark_objects: VectorQueue<ObjectReference>,
    mark_dead_objects: bool,
    mature_sweeping_in_progress: bool,
    rc: RefCountHelper<VM>,
}

impl<VM: VMBinding> ProcessDecs<VM> {
    pub fn new(decs: Vec<ObjectReference>, counter: LazySweepingJobsCounter) -> Self {
        Self {
            decs: Some(decs),
            decs_arc: None,
            new_decs: VectorQueue::default(),
            counter,
            mark_objects: VectorQueue::default(),
            mark_dead_objects: false,
            mature_sweeping_in_progress: false,
            rc: RefCountHelper::NEW,
        }
    }

    pub fn new_arc(decs: Arc<Vec<ObjectReference>>, counter: LazySweepingJobsCounter) -> Self {
        Self {
            decs: None,
            decs_arc: Some(decs),
            new_decs: VectorQueue::default(),
            counter,
            mark_objects: VectorQueue::default(),
            mark_dead_objects: false,
            mature_sweeping_in_progress: false,
            rc: RefCountHelper::NEW,
        }
    }

    fn recursive_dec(&mut self, worker: &mut GCWorker<VM>, o: ObjectReference) {
        self.new_decs.push(o);
        if self.new_decs.is_full() {
            self.flush(worker)
        }
    }

    fn new_work(&self, worker: &mut GCWorker<VM>, w: ProcessDecs<VM>) {
        worker.add_work(WorkBucketStage::Unconstrained, w);
    }

    fn flush(&mut self, worker: &mut GCWorker<VM>) {
        let mmtk = worker.mmtk;
        if !self.new_decs.is_empty() {
            let new_decs = self.new_decs.take();
            self.new_work(
                worker,
                ProcessDecs::new(new_decs, self.counter.clone_with_decs()),
            );
        }
        if !self.mark_objects.is_empty() {
            let objects = self.mark_objects.take();
            let w = LXRConcurrentTraceObjects::new(objects, mmtk);
            if LAZY_DECREMENTS {
                worker.add_work(WorkBucketStage::Unconstrained, w);
            } else {
                worker.scheduler().work_buckets[WorkBucketStage::ConcurrentResumable].add(w);
            }
        }
    }

    #[cold]
    fn process_dead_object(
        &mut self,
        worker: &mut GCWorker<VM>,
        o: ObjectReference,
        lxr: &LXR<VM>,
    ) -> bool {
        let can_poison_object = if self.mark_dead_objects {
            // If concurrent marking already claimed `o`'s mark bit (`lxr.mark(o) == false`), a
            // concurrent `LXRConcurrentTraceObjects` worker may still be reading `o`'s header to
            // scan its fields; only poison `o` when we win the mark bit first.
            lxr.mark(o)
        } else {
            // Immediately after `InitialMark` (`LAZY_DECREMENTS == true`), `self.mark_dead_objects`
            // is `false` while `lxr.concurrent_work_in_progress()` is `true` and
            // `LXRConcurrentTraceObjects` may concurrently scan objects; only poison when
            // concurrent marking is inactive.
            !lxr.concurrent_work_in_progress()
        };
        let obj_end_for_poison = if RefCountHelper::<VM>::SANITY && can_poison_object {
            Some(VM::VMObjectModel::ref_to_object_start(o) + VM::VMObjectModel::get_current_size(o))
        } else {
            None
        };
        // Recursively decrease field ref counts
        let tls = worker.tls.0;
        o.iterate_fields::<VM, _>(tls, |slot| {
            if let Some(x) = slot.load() {
                // println!(" -- rec dec {:?}.{:?} -> {:?}", o, slot, x);
                let rc = self.rc.count(x);
                if rc != MAX_REF_COUNT && rc != 0 {
                    self.recursive_dec(worker, x);
                }
                if self.mark_dead_objects && !lxr.is_marked(x) {
                    if cfg!(any(feature = "sanity", debug_assertions)) {
                        assert!(
                            x.to_raw_address().is_mapped(),
                            "Invalid object {:?}.{:?} -> {:?}: address is not mapped",
                            o,
                            slot,
                            x
                        );
                    }
                    self.mark_objects.push(x);
                    if self.mark_objects.is_full() {
                        self.flush(worker);
                    }
                }
            }
        });
        let in_ix_space = lxr.immix_space.in_space(o);
        if in_ix_space {
            // Clear the VO bit if `o` is in the immix space.
            // Note that if the object is in the LOS,
            // the VO bit will be cleared in `LargeObjectSpace::release_object`.
            #[cfg(feature = "vo_bit")]
            crate::util::metadata::vo_bit::unset_vo_bit(o);

            self.rc.unmark_straddle_object(o);
        }
        if let Some(obj_end) = obj_end_for_poison {
            if o.to_raw_address() + std::mem::size_of::<usize>() <= obj_end {
                // SAFETY: `o` is a live-allocated object whose last reference count is dropping
                // from `1 -> 0` (all epoch increments ran before decrements, so no mutator or
                // other `ProcessDecs` worker holds a reference to `o`, and `!dead` in
                // `process_decs` ensures this runs at most once even if an adjacent `RC_TABLE`
                // entry retries `fetch_update`). `can_poison_object` guarantees either concurrent
                // marking is inactive (`!lxr.concurrent_work_in_progress()`) or this worker
                // atomically claimed `o`'s mark bit first (`lxr.mark(o)`), so no concurrent
                // `LXRConcurrentTraceObjects` worker reads `o`, and `o` has not yet been released
                // to block or LOS sweeping. By `ObjectModel`'s `ObjectReference` contract,
                // `o.to_raw_address()` is non-null, word-aligned (`size_of::<usize>()`), and at or
                // after `ref_to_object_start(o)`, and the guard above ensures all
                // `size_of::<usize>()` bytes lie within `[obj_start, obj_end)`.
                unsafe { o.to_raw_address().store(0xdeadusize) };
            }
        }
        if in_ix_space {
            let block = Block::containing(o);
            lxr.add_to_possibly_dead_mature_blocks(block, false);
            false
        } else {
            true
        }
    }

    fn process_decs(&mut self, worker: &mut GCWorker<VM>, decs: &[ObjectReference], lxr: &LXR<VM>) {
        for o in decs.iter() {
            if self.rc.is_dead_or_stuck(*o)
                || (self.mature_sweeping_in_progress && !lxr.is_marked(*o))
            {
                continue;
            }
            let o = if MATURE_EVACUATION && object_forwarding::is_forwarded::<VM>(*o) {
                object_forwarding::read_forwarding_pointer::<VM>(*o)
            } else {
                *o
            };
            let mut dead = false;
            let mut is_los = false;
            let mut already_run = false;
            let result = self.rc.clone().fetch_update(o, |c| {
                if already_run {
                    log::warn!("fetch_update is re-run! o: {o}");
                } else {
                    already_run = true;
                }
                if c == 1 && !dead {
                    dead = true;
                    is_los = self.process_dead_object(worker, o, lxr);
                }
                debug_assert!(c <= MAX_REF_COUNT);
                if c == 0 || c == MAX_REF_COUNT {
                    None /* sticky */
                } else {
                    Some(c - 1)
                }
            });
            if result == Ok(1) && is_los {
                lxr.los().rc_free(o);
            }
        }
    }
}

impl<VM: VMBinding> GCWork<VM> for ProcessDecs<VM> {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, mmtk: &'static MMTK<VM>) {
        let lxr = mmtk.get_plan().downcast_ref::<LXR<VM>>().unwrap();
        self.mark_dead_objects = if LAZY_DECREMENTS {
            lxr.concurrent_work_in_progress() && lxr.previous_pause() != Some(Pause::InitialMark)
        } else {
            lxr.concurrent_work_in_progress() && lxr.current_pause() != Some(Pause::InitialMark)
        };
        self.mature_sweeping_in_progress = if LAZY_DECREMENTS {
            lxr.previous_pause() == Some(Pause::FinalMark)
                || lxr.current_pause() == Some(Pause::Full)
        } else {
            lxr.current_pause() == Some(Pause::FinalMark)
                || lxr.current_pause() == Some(Pause::Full)
        };
        if let Some(decs) = std::mem::take(&mut self.decs) {
            self.process_decs(worker, &decs, lxr);
        } else if let Some(decs) = std::mem::take(&mut self.decs_arc) {
            self.process_decs(worker, &decs, lxr);
        }
        let mut decs = vec![];
        while !self.new_decs.is_empty() {
            decs.clear();
            self.new_decs.swap(&mut decs);
            self.process_decs(worker, &decs, lxr);
        }
        self.flush(worker);
    }
}

pub struct CollectSlotRoots<VM: VMBinding> {
    base: ProcessEdgesBase<VM>,
}

impl<VM: VMBinding> CollectSlotRoots<VM> {
    pub fn new(
        slots: Vec<VM::VMSlot>,
        roots: bool,
        mmtk: &'static MMTK<VM>,
        bucket: WorkBucketStage,
    ) -> Self {
        debug_assert!(roots);
        let base = ProcessEdgesBase::new(slots, roots, mmtk, bucket);
        Self { base }
    }
}

impl<VM: VMBinding> GCWork<VM> for CollectSlotRoots<VM> {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, _mmtk: &'static MMTK<VM>) {
        if !self.slots.is_empty() {
            let lxr = self.mmtk().get_plan().downcast_ref::<LXR<VM>>().unwrap();
            let roots = std::mem::take(&mut self.slots);
            let mut w = ProcessIncs::<_, EDGE_KIND_ROOT>::new(roots, lxr);
            w.root_kind = self.root_kind;
            GCWork::do_work(&mut w, worker, self.mmtk());
        }
    }
}

impl<VM: VMBinding> Deref for CollectSlotRoots<VM> {
    type Target = ProcessEdgesBase<VM>;
    fn deref(&self) -> &Self::Target {
        &self.base
    }
}

impl<VM: VMBinding> DerefMut for CollectSlotRoots<VM> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.base
    }
}

#[cfg(feature = "non_moving_root")]
/// Collects roots reported as objects rather than as slots. Node counterpart of
/// [`CollectSlotRoots`].
pub struct CollectNodeRoots<VM: VMBinding> {
    nodes: Vec<ObjectReference>,
    _p: PhantomData<VM>,
}

#[cfg(feature = "non_moving_root")]
impl<VM: VMBinding> CollectNodeRoots<VM> {
    pub fn new(nodes: Vec<ObjectReference>) -> Self {
        Self {
            nodes,
            _p: PhantomData,
        }
    }
}

#[cfg(feature = "non_moving_root")]
impl<VM: VMBinding> GCWork<VM> for CollectNodeRoots<VM> {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, mmtk: &'static MMTK<VM>) {
        if !self.nodes.is_empty() {
            let lxr = mmtk.get_plan().downcast_ref::<LXR<VM>>().unwrap();
            let roots = std::mem::take(&mut self.nodes);
            let mut w = ProcessIncs::<_, EDGE_KIND_ROOT>::new(vec![], lxr);
            // `RootsWorkFactory` passes no `RootKind` for node roots; they are strong roots.
            w.root_kind = Some(RootKind::Strong);
            w.do_work_root_nodes(roots, worker, mmtk);
        }
    }
}
