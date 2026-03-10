use super::work_bucket::WorkBucketStage;
use super::*;
use crate::global_state::GcStatus;
use crate::plan::ObjectsClosure;
use crate::plan::VectorObjectQueue;
use crate::plan::SlotWorkFactory;
use crate::util::*;
use crate::vm::slot::Slot;
use crate::vm::*;
use crate::*;
use std::marker::PhantomData;

pub struct ScheduleCollection;

impl<VM: VMBinding> GCWork<VM> for ScheduleCollection {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, mmtk: &'static MMTK<VM>) {
        // Tell GC trigger that GC started.
        mmtk.gc_trigger.policy.on_gc_start(mmtk);

        // Determine collection kind
        let is_emergency = mmtk.state.set_collection_kind(
            mmtk.get_plan().last_collection_was_exhaustive(),
            mmtk.gc_trigger.policy.can_heap_size_grow(),
        );
        if is_emergency {
            mmtk.get_plan().notify_emergency_collection();
        }
        // Set to GcPrepare
        mmtk.set_gc_status(GcStatus::GcPrepare);

        // Let the plan to schedule collection work
        mmtk.get_plan().schedule_collection(worker.scheduler());
    }
}

/// The global GC Preparation Work
/// This work packet invokes prepare() for the plan (which will invoke prepare() for each space), and
/// pushes work packets for preparing mutators and collectors.
/// We should only have one such work packet per GC, before any actual GC work starts.
/// We assume this work packet is the only running work packet that accesses plan, and there should
/// be no other concurrent work packet that accesses plan (read or write). Otherwise, there may
/// be a race condition.
pub struct Prepare<C: GCWorkContext> {
    pub plan: *const C::PlanType,
}

unsafe impl<C: GCWorkContext> Send for Prepare<C> {}

impl<C: GCWorkContext> Prepare<C> {
    pub fn new(plan: *const C::PlanType) -> Self {
        Self { plan }
    }
}

impl<C: GCWorkContext> GCWork<C::VM> for Prepare<C> {
    fn do_work(&mut self, worker: &mut GCWorker<C::VM>, mmtk: &'static MMTK<C::VM>) {
        trace!("Prepare Global");
        // We assume this is the only running work packet that accesses plan at the point of execution
        let plan_mut: &mut C::PlanType = unsafe { &mut *(self.plan as *const _ as *mut _) };
        plan_mut.prepare(worker.tls);

        if plan_mut.constraints().needs_prepare_mutator {
            let prepare_mutator_packets = <C::VM as VMBinding>::VMActivePlan::mutators()
                .map(|mutator| Box::new(PrepareMutator::<C::VM>::new(mutator)) as _)
                .collect::<Vec<_>>();
            // Just in case the VM binding is inconsistent about the number of mutators and the actual mutator list.
            debug_assert_eq!(
                prepare_mutator_packets.len(),
                <C::VM as VMBinding>::VMActivePlan::number_of_mutators()
            );
            mmtk.scheduler.work_buckets[WorkBucketStage::Prepare].bulk_add(prepare_mutator_packets);
        }

        for w in &mmtk.scheduler.worker_group.workers_shared {
            let result = w.designated_work.push(Box::new(PrepareCollector));
            debug_assert!(result.is_ok());
        }
    }
}

/// The mutator GC Preparation Work
pub struct PrepareMutator<VM: VMBinding> {
    // The mutator reference has static lifetime.
    // It is safe because the actual lifetime of this work-packet will not exceed the lifetime of a GC.
    pub mutator: &'static mut Mutator<VM>,
}

impl<VM: VMBinding> PrepareMutator<VM> {
    pub fn new(mutator: &'static mut Mutator<VM>) -> Self {
        Self { mutator }
    }
}

impl<VM: VMBinding> GCWork<VM> for PrepareMutator<VM> {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, _mmtk: &'static MMTK<VM>) {
        trace!("Prepare Mutator");
        self.mutator.prepare(worker.tls);
    }
}

/// The collector GC Preparation Work
#[derive(Default)]
pub struct PrepareCollector;

impl<VM: VMBinding> GCWork<VM> for PrepareCollector {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, mmtk: &'static MMTK<VM>) {
        trace!("Prepare Collector");
        worker.get_copy_context_mut().prepare();
        mmtk.get_plan().prepare_worker(worker);
    }
}

/// The global GC release Work
/// This work packet invokes release() for the plan (which will invoke release() for each space), and
/// pushes work packets for releasing mutators and collectors.
/// We should only have one such work packet per GC, after all actual GC work ends.
/// We assume this work packet is the only running work packet that accesses plan, and there should
/// be no other concurrent work packet that accesses plan (read or write). Otherwise, there may
/// be a race condition.
pub struct Release<C: GCWorkContext> {
    pub plan: *const C::PlanType,
}

impl<C: GCWorkContext> Release<C> {
    pub fn new(plan: *const C::PlanType) -> Self {
        Self { plan }
    }
}

unsafe impl<C: GCWorkContext> Send for Release<C> {}

impl<C: GCWorkContext + 'static> GCWork<C::VM> for Release<C> {
    fn do_work(&mut self, worker: &mut GCWorker<C::VM>, mmtk: &'static MMTK<C::VM>) {
        trace!("Release Global");

        mmtk.gc_trigger.policy.on_gc_release(mmtk);
        // We assume this is the only running work packet that accesses plan at the point of execution

        let plan_mut: &mut C::PlanType = unsafe { &mut *(self.plan as *const _ as *mut _) };
        plan_mut.release(worker.tls);

        let release_mutator_packets = <C::VM as VMBinding>::VMActivePlan::mutators()
            .map(|mutator| Box::new(ReleaseMutator::<C::VM>::new(mutator)) as _)
            .collect::<Vec<_>>();
        // Just in case the VM binding is inconsistent about the number of mutators and the actual mutator list.
        debug_assert_eq!(
            release_mutator_packets.len(),
            <C::VM as VMBinding>::VMActivePlan::number_of_mutators()
        );
        mmtk.scheduler.work_buckets[WorkBucketStage::Release].bulk_add(release_mutator_packets);

        for w in &mmtk.scheduler.worker_group.workers_shared {
            let result = w.designated_work.push(Box::new(ReleaseCollector));
            debug_assert!(result.is_ok());
        }
    }
}

/// The mutator release Work
pub struct ReleaseMutator<VM: VMBinding> {
    // The mutator reference has static lifetime.
    // It is safe because the actual lifetime of this work-packet will not exceed the lifetime of a GC.
    pub mutator: &'static mut Mutator<VM>,
}

impl<VM: VMBinding> ReleaseMutator<VM> {
    pub fn new(mutator: &'static mut Mutator<VM>) -> Self {
        Self { mutator }
    }
}

impl<VM: VMBinding> GCWork<VM> for ReleaseMutator<VM> {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, _mmtk: &'static MMTK<VM>) {
        trace!("Release Mutator");
        self.mutator.release(worker.tls);
    }
}

/// The collector release Work
#[derive(Default)]
pub struct ReleaseCollector;

impl<VM: VMBinding> GCWork<VM> for ReleaseCollector {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, _mmtk: &'static MMTK<VM>) {
        trace!("Release Collector");
        worker.get_copy_context_mut().release();
    }
}

/// Stop all mutators
///
/// TODO: Smaller work granularity
#[derive(Default)]
pub struct StopMutators<C: GCWorkContext> {
    /// If this is true, we skip creating [`ScanMutatorRoots`] work packets for mutators.
    /// By default, this is false.
    skip_mutator_roots: bool,
    /// Flush mutators once they are stopped. By default this is false. [`ScanMutatorRoots`] will flush mutators.
    flush_mutator: bool,
    phantom: PhantomData<C>,
}

impl<C: GCWorkContext> StopMutators<C> {
    pub fn new() -> Self {
        Self {
            skip_mutator_roots: false,
            flush_mutator: false,
            phantom: PhantomData,
        }
    }

    /// Create a `StopMutators` work packet that does not create `ScanMutatorRoots` work packets for mutators, and will simply flush mutators.
    pub fn new_no_scan_roots() -> Self {
        Self {
            skip_mutator_roots: true,
            flush_mutator: true,
            phantom: PhantomData,
        }
    }
}

impl<C: GCWorkContext> GCWork<C::VM> for StopMutators<C> {
    fn do_work(&mut self, worker: &mut GCWorker<C::VM>, mmtk: &'static MMTK<C::VM>) {
        trace!("stop_all_mutators start");
        mmtk.state.prepare_for_stack_scanning();
        <C::VM as VMBinding>::VMCollection::stop_all_mutators(worker.tls, |mutator| {
            // TODO: The stack scanning work won't start immediately, as the `Prepare` bucket is not opened yet (the bucket is opened in notify_mutators_paused).
            // Should we push to Unconstrained instead?

            if self.flush_mutator {
                mutator.flush();
            }
            if !self.skip_mutator_roots {
                mmtk.scheduler.work_buckets[WorkBucketStage::Prepare]
                    .add(ScanMutatorRoots::<C>(mutator));
            }
        });
        trace!("stop_all_mutators end");
        mmtk.get_plan().notify_mutators_paused(&mmtk.scheduler);
        mmtk.scheduler.notify_mutators_paused(mmtk);
        mmtk.scheduler.work_buckets[WorkBucketStage::Prepare].add(ScanVMSpecificRoots::<C>::new());
    }
}

/// VM binding to process weak references.
///
/// NOTE: This will replace `{Soft,Weak,Phantom}RefProcessing` and `Finalization` in the future.
pub struct VMProcessWeakRefs<VM: VMBinding, T: TracePolicy<VM>> {
    phantom_data: PhantomData<(VM, T)>,
}

impl<VM: VMBinding, T: TracePolicy<VM>> VMProcessWeakRefs<VM, T> {
    pub fn new() -> Self {
        Self {
            phantom_data: PhantomData,
        }
    }
}

impl<VM: VMBinding, T: TracePolicy<VM>> GCWork<VM> for VMProcessWeakRefs<VM, T> {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, _mmtk: &'static MMTK<VM>) {
        trace!("VMProcessWeakRefs");

        let stage = WorkBucketStage::VMRefClosure;

        let need_to_repeat = {
            let tracer_factory = GCTracerContext::<VM, T> {
                stage,
                _phantom: PhantomData,
            };
            <VM as VMBinding>::VMScanning::process_weak_refs(worker, tracer_factory)
        };

        if need_to_repeat {
            // Schedule Self as the new sentinel so we'll call `process_weak_refs` again after the
            // current transitive closure.
            let new_self = Box::new(Self::new());

            worker.scheduler().work_buckets[stage].set_sentinel(new_self);
        }
    }
}

/// Delegate to the VM binding for forwarding weak references.
///
/// Some VMs (e.g. v8) do not have a Java-like global weak reference storage, and the
/// processing of those weakrefs may be more complex. For such case, we delegate to the
/// VM binding to process weak references.
///
/// NOTE: This will replace `RefForwarding` and `ForwardFinalization` in the future.
pub struct VMForwardWeakRefs<VM: VMBinding, T: TracePolicy<VM>> {
    phantom_data: PhantomData<(VM, T)>,
}

impl<VM: VMBinding, T: TracePolicy<VM>> VMForwardWeakRefs<VM, T> {
    pub fn new() -> Self {
        Self {
            phantom_data: PhantomData,
        }
    }
}

impl<VM: VMBinding, T: TracePolicy<VM>> GCWork<VM> for VMForwardWeakRefs<VM, T> {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, _mmtk: &'static MMTK<VM>) {
        trace!("VMForwardWeakRefs");

        let stage = WorkBucketStage::VMRefForwarding;

        let tracer_factory = GCTracerContext::<VM, T> {
            stage,
            _phantom: PhantomData,
        };
        <VM as VMBinding>::VMScanning::forward_weak_refs(worker, tracer_factory)
    }
}

/// This work packet calls `Collection::post_forwarding`.
///
/// NOTE: This will replace `RefEnqueue` in the future.
///
/// NOTE: Although this work packet runs in parallel with the `Release` work packet, it does not
/// access the `Plan` instance.
#[derive(Default)]
pub struct VMPostForwarding<VM: VMBinding> {
    phantom_data: PhantomData<VM>,
}

impl<VM: VMBinding> GCWork<VM> for VMPostForwarding<VM> {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, _mmtk: &'static MMTK<VM>) {
        trace!("VMPostForwarding start");
        <VM as VMBinding>::VMCollection::post_forwarding(worker.tls);
        trace!("VMPostForwarding end");
    }
}

pub struct ScanMutatorRoots<C: GCWorkContext>(pub &'static mut Mutator<C::VM>);

impl<C: GCWorkContext> GCWork<C::VM> for ScanMutatorRoots<C> {
    fn do_work(&mut self, worker: &mut GCWorker<C::VM>, mmtk: &'static MMTK<C::VM>) {
        trace!("ScanMutatorRoots for mutator {:?}", self.0.get_tls());
        let mutators = <C::VM as VMBinding>::VMActivePlan::number_of_mutators();
        let factory = GCRootsWorkFactory::<
            C::VM,
            C::DefaultTracePolicy,
            C::PinningTracePolicy,
        >::new(mmtk);
        <C::VM as VMBinding>::VMScanning::scan_roots_in_mutator_thread(
            worker.tls,
            unsafe { &mut *(self.0 as *mut _) },
            factory,
        );
        self.0.flush();

        if mmtk.state.inform_stack_scanned(mutators) {
            <C::VM as VMBinding>::VMScanning::notify_initial_thread_scan_complete(
                false, worker.tls,
            );
            mmtk.set_gc_status(GcStatus::GcProper);
        }
    }
}

#[derive(Default)]
pub struct ScanVMSpecificRoots<C: GCWorkContext>(PhantomData<C>);

impl<C: GCWorkContext> ScanVMSpecificRoots<C> {
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

impl<C: GCWorkContext> GCWork<C::VM> for ScanVMSpecificRoots<C> {
    fn do_work(&mut self, worker: &mut GCWorker<C::VM>, mmtk: &'static MMTK<C::VM>) {
        trace!("ScanStaticRoots");
        let factory = GCRootsWorkFactory::<
            C::VM,
            C::DefaultTracePolicy,
            C::PinningTracePolicy,
        >::new(mmtk);
        <C::VM as VMBinding>::VMScanning::scan_vm_specific_roots(worker.tls, factory);
    }
}





/// For USDT tracepoints for roots.
/// Keep in sync with `tools/tracing/timeline/visualize.py`.
#[repr(usize)]
enum RootsKind {
    NORMAL = 0,
    PINNING = 1,
    TPINNING = 2,
}


// ============================================================================
// New TracePolicy-based architecture
// ============================================================================

use crate::plan::GenerationalPlanExt;

/// The plan-specific tracing policy. This is the **only** thing a plan needs to provide.
///
/// A `TracePolicy` encapsulates how to trace a single object (i.e., mark or copy it),
/// and optionally what to do after scanning each object. All other concerns —
/// slot processing, node queue management, work packet lifecycle, scan work creation —
/// are handled generically by [`GCProcessEdges`] and [`GCScanObjects`].
///
/// This trait replaces the overloaded [`ProcessEdgesWork`] trait for new code.
pub trait TracePolicy<VM: VMBinding>: Send + Clone + 'static {
    /// Whether this trace may move objects (determines if slots need updating).
    fn may_move_objects(&self) -> bool;

    /// Trace one object. If this is the first visit, enqueue it in `queue`.
    fn trace_object(
        &self,
        queue: &mut VectorObjectQueue,
        object: ObjectReference,
        worker: &mut GCWorker<VM>,
    ) -> ObjectReference;

    /// Hook called after scanning each object (e.g., Immix line marking).
    /// The default is a no-op.
    fn post_scan_object(&self, _object: ObjectReference) {}

    /// Construct this policy from an MMTK instance.
    /// Used when creating new work packets internally.
    fn from_mmtk(mmtk: &'static MMTK<VM>) -> Self;
}

use crate::plan::Plan;
use crate::plan::PlanTraceObject;
use crate::policy::gc_work::TraceKind;

/// A `TracePolicy` that traces objects using [`PlanTraceObject::trace_object`] with
/// a specific [`TraceKind`]. This is the most common trace policy, used by most plans.
///
/// Replaces `PlanProcessEdges<VM, P, KIND>`.
pub struct MatureTracePolicy<
    VM: VMBinding,
    P: Plan<VM = VM> + PlanTraceObject<VM>,
    const KIND: TraceKind,
> {
    plan: &'static P,
    _phantom: PhantomData<VM>,
}

impl<VM: VMBinding, P: Plan<VM = VM> + PlanTraceObject<VM>, const KIND: TraceKind> Clone
    for MatureTracePolicy<VM, P, KIND>
{
    fn clone(&self) -> Self {
        Self {
            plan: self.plan,
            _phantom: PhantomData,
        }
    }
}

impl<VM: VMBinding, P: Plan<VM = VM> + PlanTraceObject<VM>, const KIND: TraceKind>
    TracePolicy<VM> for MatureTracePolicy<VM, P, KIND>
{
    fn may_move_objects(&self) -> bool {
        P::may_move_objects::<KIND>()
    }

    fn trace_object(
        &self,
        queue: &mut VectorObjectQueue,
        object: ObjectReference,
        worker: &mut GCWorker<VM>,
    ) -> ObjectReference {
        self.plan.trace_object::<VectorObjectQueue, KIND>(queue, object, worker)
    }

    fn post_scan_object(&self, object: ObjectReference) {
        self.plan.post_scan_object(object);
    }

    fn from_mmtk(mmtk: &'static MMTK<VM>) -> Self {
        let plan = mmtk.get_plan().downcast_ref::<P>().unwrap();
        Self {
            plan,
            _phantom: PhantomData,
        }
    }
}

/// A `TracePolicy` for nursery GC in generational plans.
/// Traces using [`GenerationalPlanExt::trace_object_nursery`].
///
/// Replaces `GenNurseryProcessEdges<VM, P, KIND>`.
pub struct NurseryTracePolicy<
    VM: VMBinding,
    P: GenerationalPlanExt<VM> + PlanTraceObject<VM>,
    const KIND: TraceKind,
> {
    plan: &'static P,
    _phantom: PhantomData<VM>,
}

impl<VM: VMBinding, P: GenerationalPlanExt<VM> + PlanTraceObject<VM>, const KIND: TraceKind> Clone
    for NurseryTracePolicy<VM, P, KIND>
{
    fn clone(&self) -> Self {
        Self {
            plan: self.plan,
            _phantom: PhantomData,
        }
    }
}

impl<VM: VMBinding, P: GenerationalPlanExt<VM> + PlanTraceObject<VM>, const KIND: TraceKind>
    TracePolicy<VM> for NurseryTracePolicy<VM, P, KIND>
{
    fn may_move_objects(&self) -> bool {
        // Nursery always moves objects (copies from nursery to mature).
        true
    }

    fn trace_object(
        &self,
        queue: &mut VectorObjectQueue,
        object: ObjectReference,
        worker: &mut GCWorker<VM>,
    ) -> ObjectReference {
        self.plan.trace_object_nursery::<VectorObjectQueue, KIND>(queue, object, worker)
    }

    fn post_scan_object(&self, object: ObjectReference) {
        self.plan.post_scan_object(object);
    }

    fn from_mmtk(mmtk: &'static MMTK<VM>) -> Self {
        let plan = mmtk.get_plan().downcast_ref::<P>().unwrap();
        Self {
            plan,
            _phantom: PhantomData,
        }
    }
}

/// A `TracePolicy` that panics at runtime. Used for plans that don't support
/// pinning or transitive pinning roots.
///
/// Replaces `UnsupportedProcessEdges<VM>`.
#[derive(Default)]
pub struct UnsupportedTracePolicy<VM: VMBinding> {
    _phantom: PhantomData<VM>,
}

impl<VM: VMBinding> Clone for UnsupportedTracePolicy<VM> {
    fn clone(&self) -> Self {
        Self {
            _phantom: PhantomData,
        }
    }
}

impl<VM: VMBinding> TracePolicy<VM> for UnsupportedTracePolicy<VM> {
    fn may_move_objects(&self) -> bool {
        panic!("unsupported!")
    }

    fn trace_object(
        &self,
        _queue: &mut VectorObjectQueue,
        _object: ObjectReference,
        _worker: &mut GCWorker<VM>,
    ) -> ObjectReference {
        panic!("unsupported!")
    }

    fn from_mmtk(_mmtk: &'static MMTK<VM>) -> Self {
        panic!("unsupported!")
    }
}

/// Generic edge-processing work packet. Replaces `PlanProcessEdges`, `GenNurseryProcessEdges`,
/// `SFTProcessEdges`, and the entire `ProcessEdgesBase + Deref/DerefMut` pattern.
///
/// This struct owns the plumbing (slots, node queue, worker pointer) and delegates
/// only the tracing decision to its `TracePolicy`.
pub struct GCProcessEdges<VM: VMBinding, T: TracePolicy<VM>> {
    policy: T,
    pub(crate) slots: Vec<VM::VMSlot>,
    pub(crate) nodes: VectorObjectQueue,
    mmtk: &'static MMTK<VM>,
    worker: *mut GCWorker<VM>,
    pub(crate) roots: bool,
    pub(crate) bucket: WorkBucketStage,
}

unsafe impl<VM: VMBinding, T: TracePolicy<VM>> Send for GCProcessEdges<VM, T> {}

impl<VM: VMBinding, T: TracePolicy<VM>> GCProcessEdges<VM, T> {
    /// Create a new edge-processing work packet.
    pub fn new(
        slots: Vec<VM::VMSlot>,
        roots: bool,
        mmtk: &'static MMTK<VM>,
        bucket: WorkBucketStage,
    ) -> Self {
        let policy = T::from_mmtk(mmtk);
        Self::new_with_policy(slots, roots, policy, mmtk, bucket)
    }

    /// Create with an explicit policy (used when the caller already has one).
    pub fn new_with_policy(
        slots: Vec<VM::VMSlot>,
        roots: bool,
        policy: T,
        mmtk: &'static MMTK<VM>,
        bucket: WorkBucketStage,
    ) -> Self {
        #[cfg(feature = "extreme_assertions")]
        if crate::util::slot_logger::should_check_duplicate_slots(mmtk.get_plan()) {
            for slot in &slots {
                mmtk.slot_logger.log_slot(*slot);
            }
        }
        Self {
            policy,
            slots,
            nodes: VectorObjectQueue::new(),
            mmtk,
            worker: std::ptr::null_mut(),
            roots,
            bucket,
        }
    }

    /// Create a tracer-only instance (no slots). Used by reference/finalizable processing.
    pub fn new_tracer(mmtk: &'static MMTK<VM>, bucket: WorkBucketStage) -> Self {
        Self::new(vec![], false, mmtk, bucket)
    }

    pub fn set_worker(&mut self, worker: &mut GCWorker<VM>) {
        self.worker = worker;
    }

    pub fn worker(&self) -> &'static mut GCWorker<VM> {
        unsafe { &mut *self.worker }
    }

    #[cfg(feature = "sanity")]
    pub fn mmtk(&self) -> &'static MMTK<VM> {
        self.mmtk
    }

    /// Trace an object using the policy.
    pub fn trace_object(&mut self, object: ObjectReference) -> ObjectReference {
        let worker = self.worker();
        self.policy.trace_object(&mut self.nodes, object, worker)
    }

    /// Pop all buffered nodes.
    pub fn pop_nodes(&mut self) -> Vec<ObjectReference> {
        self.nodes.take()
    }

    /// Process a single slot: load, trace, update if moved.
    fn process_slot(&mut self, slot: VM::VMSlot) {
        let Some(object) = slot.load() else { return };
        let new_object = self.trace_object(object);
        if self.policy.may_move_objects() && new_object != object {
            slot.store(new_object);
        }
    }

    /// Process all slots in the buffer.
    fn process_slots(&mut self) {
        probe!(mmtk, process_slots, self.slots.len(), self.roots);
        for i in 0..self.slots.len() {
            self.process_slot(self.slots[i]);
        }
    }

    /// Create a scan-objects work packet for the given nodes.
    pub fn create_scan_work(&self, nodes: Vec<ObjectReference>) -> GCScanObjects<VM, T> {
        GCScanObjects::new(self.policy.clone(), nodes, false, self.bucket)
    }

    /// Flush buffered nodes into a scan-objects work packet.
    pub fn flush(&mut self) {
        let nodes = self.pop_nodes();
        if !nodes.is_empty() {
            let work_packet = self.create_scan_work(nodes);
            // Execute immediately for better locality (matching ProcessEdgesWork::SCAN_OBJECTS_IMMEDIATELY).
            work_packet.do_work_inner(self.worker(), self.mmtk);
        }
    }

    /// Cache roots for sanity GC.
    #[cfg(feature = "sanity")]
    fn cache_roots_for_sanity_gc(&mut self) {
        assert!(self.roots);
        self.mmtk()
            .sanity_checker
            .lock()
            .unwrap()
            .add_root_slots(self.slots.clone());
    }
}

impl<VM: VMBinding, T: TracePolicy<VM>> GCWork<VM> for GCProcessEdges<VM, T> {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, _mmtk: &'static MMTK<VM>) {
        self.set_worker(worker);
        self.process_slots();
        if !self.nodes.is_empty() {
            self.flush();
        }
        #[cfg(feature = "sanity")]
        if self.roots && !_mmtk.is_in_sanity() {
            self.cache_roots_for_sanity_gc();
        }
        trace!("GCProcessEdges End");
    }
}

/// `SlotWorkFactory` impl for `GCProcessEdges` — creates new `GCProcessEdges` work packets.
impl<VM: VMBinding, T: TracePolicy<VM>> SlotWorkFactory<VM> for GCProcessEdges<VM, T> {
    fn add_slot_processing_work(
        worker: &mut GCWorker<VM>,
        slots: Vec<VM::VMSlot>,
        mmtk: &'static MMTK<VM>,
        bucket: WorkBucketStage,
    ) {
        worker.add_work(bucket, GCProcessEdges::<VM, T>::new(slots, false, mmtk, bucket));
    }
}

/// Wraps a `GCProcessEdges` as an `ObjectTracer` for use in non-slot-enqueuing scanning
/// and weak reference processing.
pub(crate) struct GCTracer<VM: VMBinding, T: TracePolicy<VM>> {
    edges: GCProcessEdges<VM, T>,
    stage: WorkBucketStage,
}

impl<VM: VMBinding, T: TracePolicy<VM>> ObjectTracer for GCTracer<VM, T> {
    fn trace_object(&mut self, object: ObjectReference) -> ObjectReference {
        let result = self.edges.trace_object(object);
        // Flush if the node buffer is full
        if self.edges.nodes.is_full() {
            self.flush();
        }
        result
    }
}

impl<VM: VMBinding, T: TracePolicy<VM>> GCTracer<VM, T> {
    fn flush(&mut self) {
        let nodes = self.edges.pop_nodes();
        if !nodes.is_empty() {
            let work_packet = self.edges.create_scan_work(nodes);
            let worker = self.edges.worker();
            worker.scheduler().work_buckets[self.stage].add(work_packet);
        }
    }

    pub fn flush_if_not_empty(&mut self) {
        if !self.edges.nodes.is_empty() {
            self.flush();
        }
    }
}

/// `ObjectTracerContext` for `GCProcessEdges`, replacing `ProcessEdgesWorkTracerContext`
/// for TracePolicy-based code.
pub(crate) struct GCTracerContext<VM: VMBinding, T: TracePolicy<VM>> {
    pub(crate) stage: WorkBucketStage,
    pub(crate) _phantom: PhantomData<(VM, T)>,
}

impl<VM: VMBinding, T: TracePolicy<VM>> Clone for GCTracerContext<VM, T> {
    fn clone(&self) -> Self {
        Self {
            stage: self.stage,
            _phantom: PhantomData,
        }
    }
}

impl<VM: VMBinding, T: TracePolicy<VM>> ObjectTracerContext<VM> for GCTracerContext<VM, T> {
    type TracerType = GCTracer<VM, T>;

    fn with_tracer<R, F>(&self, worker: &mut GCWorker<VM>, func: F) -> R
    where
        F: FnOnce(&mut Self::TracerType) -> R,
    {
        let mmtk = worker.mmtk;
        let mut edges = GCProcessEdges::<VM, T>::new_tracer(mmtk, self.stage);
        edges.set_worker(worker);

        let mut tracer = GCTracer {
            edges,
            stage: self.stage,
        };

        let result = func(&mut tracer);
        tracer.flush_if_not_empty();
        result
    }
}

/// Generic object-scanning work packet. Replaces both `ScanObjects<E>` and `PlanScanObjects<E, P>`,
/// and eliminates the `ScanObjectsWork` trait entirely.
///
/// This single struct handles:
/// - Slot-enqueuing scanning (via `ObjectsClosure`)
/// - Node-enqueuing scanning (via `ProcessEdgesWorkTracerContext`)
/// - Post-scan hooks (via `TracePolicy::post_scan_object`)
pub struct GCScanObjects<VM: VMBinding, T: TracePolicy<VM>> {
    policy: T,
    buffer: Vec<ObjectReference>,
    #[allow(dead_code)]
    concurrent: bool,
    bucket: WorkBucketStage,
    _phantom: PhantomData<VM>,
}

impl<VM: VMBinding, T: TracePolicy<VM>> GCScanObjects<VM, T> {
    pub fn new(
        policy: T,
        buffer: Vec<ObjectReference>,
        concurrent: bool,
        bucket: WorkBucketStage,
    ) -> Self {
        Self {
            policy,
            buffer,
            concurrent,
            bucket,
            _phantom: PhantomData,
        }
    }

    /// Do the actual scanning work. This is also called from `GCProcessEdges::flush`
    /// for the "scan immediately" optimization.
    pub(crate) fn do_work_inner(&self, worker: &mut GCWorker<VM>, mmtk: &'static MMTK<VM>) {
        let tls = worker.tls;
        let objects_to_scan = &self.buffer;

        // Scan objects that support slot-enqueuing
        let mut scan_later = vec![];
        {
            let mut closure = ObjectsClosure::<VM, GCProcessEdges<VM, T>>::new(worker, self.bucket);

            if crate::util::rust_util::unlikely(*mmtk.get_options().count_live_bytes_in_gc) {
                let mut live_bytes_stats = closure.worker.shared.live_bytes_per_space.borrow_mut();
                for object in objects_to_scan.iter().copied() {
                    crate::scheduler::worker::GCWorkerShared::<VM>::increase_live_bytes(
                        &mut live_bytes_stats,
                        object,
                    );
                }
            }

            for object in objects_to_scan.iter().copied() {
                if <VM as VMBinding>::VMScanning::support_slot_enqueuing(tls, object) {
                    trace!("Scan object (slot) {}", object);
                    <VM as VMBinding>::VMScanning::scan_object(tls, object, &mut closure);
                    self.policy.post_scan_object(object);
                } else {
                    scan_later.push(object);
                }
            }
        }

        let total_objects = objects_to_scan.len();
        let scan_and_trace = scan_later.len();
        probe!(mmtk, scan_objects, total_objects, scan_and_trace);

        // Handle objects that don't support slot-enqueuing
        if !scan_later.is_empty() {
            let object_tracer_context = GCTracerContext::<VM, T> {
                stage: self.bucket,
                _phantom: PhantomData,
            };

            object_tracer_context.with_tracer(worker, |object_tracer| {
                for object in scan_later.iter().copied() {
                    trace!("Scan object (node) {}", object);
                    <VM as VMBinding>::VMScanning::scan_object_and_trace_edges(
                        tls,
                        object,
                        object_tracer,
                    );
                    self.policy.post_scan_object(object);
                }
            });
        }
    }
}

impl<VM: VMBinding, T: TracePolicy<VM>> GCWork<VM> for GCScanObjects<VM, T> {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, mmtk: &'static MMTK<VM>) {
        trace!("GCScanObjects");
        self.do_work_inner(worker, mmtk);
        trace!("GCScanObjects End");
    }
}

/// An implementation of `RootsWorkFactory` based on `TracePolicy`, replacing
/// `ProcessEdgesWorkRootsWorkFactory`. `DT` is the default trace policy (for normal roots),
/// `PT` is the pinning trace policy (for pinning/transitive-pinning roots).
pub(crate) struct GCRootsWorkFactory<
    VM: VMBinding,
    DT: TracePolicy<VM>,
    PT: TracePolicy<VM>,
> {
    mmtk: &'static MMTK<VM>,
    _phantom: PhantomData<(DT, PT)>,
}

impl<VM: VMBinding, DT: TracePolicy<VM>, PT: TracePolicy<VM>> Clone
    for GCRootsWorkFactory<VM, DT, PT>
{
    fn clone(&self) -> Self {
        Self {
            mmtk: self.mmtk,
            _phantom: PhantomData,
        }
    }
}

impl<VM: VMBinding, DT: TracePolicy<VM>, PT: TracePolicy<VM>>
    GCRootsWorkFactory<VM, DT, PT>
{
    pub fn new(mmtk: &'static MMTK<VM>) -> Self {
        Self {
            mmtk,
            _phantom: PhantomData,
        }
    }
}

impl<VM: VMBinding, DT: TracePolicy<VM>, PT: TracePolicy<VM>>
    RootsWorkFactory<VM::VMSlot> for GCRootsWorkFactory<VM, DT, PT>
{
    fn create_process_roots_work(&mut self, slots: Vec<VM::VMSlot>) {
        probe!(mmtk, roots, RootsKind::NORMAL, slots.len());
        crate::memory_manager::add_work_packet(
            self.mmtk,
            WorkBucketStage::Closure,
            GCProcessEdges::<VM, DT>::new(slots, true, self.mmtk, WorkBucketStage::Closure),
        );
    }

    fn create_process_pinning_roots_work(&mut self, nodes: Vec<ObjectReference>) {
        probe!(mmtk, roots, RootsKind::PINNING, nodes.len());
        crate::memory_manager::add_work_packet(
            self.mmtk,
            WorkBucketStage::PinningRootsTrace,
            GCProcessRootNodes::<VM, PT, DT>::new(nodes, WorkBucketStage::Closure),
        );
    }

    fn create_process_tpinning_roots_work(&mut self, nodes: Vec<ObjectReference>) {
        probe!(mmtk, roots, RootsKind::TPINNING, nodes.len());
        crate::memory_manager::add_work_packet(
            self.mmtk,
            WorkBucketStage::TPinningClosure,
            GCProcessRootNodes::<VM, PT, PT>::new(nodes, WorkBucketStage::TPinningClosure),
        );
    }
}

/// Process root nodes using TracePolicy. Replaces `ProcessRootNodes<VM, R2OPE, O2OPE>`.
///
/// `PT` (Pinning Trace) is the policy used for tracing root objects (must not move objects).
/// `DT` (Default Trace) is the policy used for scanning the children of root objects.
///
/// For pinning roots: PT = PinningTracePolicy, DT = DefaultTracePolicy
/// For transitive pinning roots: PT = PinningTracePolicy, DT = PinningTracePolicy
pub(crate) struct GCProcessRootNodes<
    VM: VMBinding,
    PT: TracePolicy<VM>,
    DT: TracePolicy<VM>,
> {
    _phantom: PhantomData<(VM, PT, DT)>,
    roots: Vec<ObjectReference>,
    bucket: WorkBucketStage,
}

impl<VM: VMBinding, PT: TracePolicy<VM>, DT: TracePolicy<VM>>
    GCProcessRootNodes<VM, PT, DT>
{
    pub fn new(nodes: Vec<ObjectReference>, bucket: WorkBucketStage) -> Self {
        Self {
            _phantom: PhantomData,
            roots: nodes,
            bucket,
        }
    }
}

impl<VM: VMBinding, PT: TracePolicy<VM>, DT: TracePolicy<VM>> GCWork<VM>
    for GCProcessRootNodes<VM, PT, DT>
{
    fn do_work(&mut self, worker: &mut GCWorker<VM>, mmtk: &'static MMTK<VM>) {
        trace!("GCProcessRootNodes");

        #[cfg(feature = "sanity")]
        {
            if !mmtk.is_in_sanity() {
                mmtk.sanity_checker
                    .lock()
                    .unwrap()
                    .add_root_nodes(self.roots.clone());
            }
        }

        let num_roots = self.roots.len();

        // Trace root objects using the pinning policy (must not move objects).
        let root_objects_to_scan = {
            let mut process_edges =
                GCProcessEdges::<VM, PT>::new(vec![], true, mmtk, WorkBucketStage::PinningRootsTrace);
            process_edges.set_worker(worker);

            for object in self.roots.iter().copied() {
                let new_object = process_edges.trace_object(object);
                debug_assert_eq!(
                    object, new_object,
                    "Object moved while tracing root unmovable root object: {} -> {}",
                    object, new_object
                );
            }

            process_edges.pop_nodes()
        };

        let num_enqueued_nodes = root_objects_to_scan.len();
        probe!(mmtk, process_root_nodes, num_roots, num_enqueued_nodes);

        // Create scan work using the default policy for children.
        if !root_objects_to_scan.is_empty() {
            let policy = DT::from_mmtk(mmtk);
            let work = GCScanObjects::<VM, DT>::new(policy, root_objects_to_scan, false, self.bucket);
            crate::memory_manager::add_work_packet(mmtk, self.bucket, work);
        }

        trace!("GCProcessRootNodes End");
    }
}
