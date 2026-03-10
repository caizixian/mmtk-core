use super::work_bucket::WorkBucketStage;
use super::*;
use crate::global_state::GcStatus;

use crate::plan::{TracePolicy, VectorObjectQueue, VectorQueue};
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

/// Delegate to the VM binding for weak reference processing.
///
/// Some VMs (e.g. v8) do not have a Java-like global weak reference storage, and the
/// processing of those weakrefs may be more complex. For such case, we delegate to the
/// VM binding to process weak references.
///
/// NOTE: This will replace `{Soft,Weak,Phantom}RefProcessing` and `Finalization` in the future.
pub struct VMProcessWeakRefs<VM: VMBinding, T: TracePolicy<VM>> {
    _phantom: PhantomData<(VM, T)>,
}

impl<VM: VMBinding, T: TracePolicy<VM>> VMProcessWeakRefs<VM, T> {
    pub fn new() -> Self {
        Self {
            _phantom: PhantomData,
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
            VM::VMScanning::process_weak_refs(worker, tracer_factory)
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
    _phantom: PhantomData<(VM, T)>,
}

impl<VM: VMBinding, T: TracePolicy<VM>> VMForwardWeakRefs<VM, T> {
    pub fn new() -> Self {
        Self {
            _phantom: PhantomData,
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
        VM::VMScanning::forward_weak_refs(worker, tracer_factory)
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
        let factory =
            GCRootsWorkFactory::<C::VM, C::DefaultTracePolicy, C::PinningTracePolicy>::new(mmtk);
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
        let factory =
            GCRootsWorkFactory::<C::VM, C::DefaultTracePolicy, C::PinningTracePolicy>::new(mmtk);
        <C::VM as VMBinding>::VMScanning::scan_vm_specific_roots(worker.tls, factory);
    }
}

/// A short-hand for `<E::VM as VMBinding>::VMSlot`.

/// An abstract trait for work packets that process object graph edges.  Its method
/// [`ProcessEdgesWork::trace_object`] traces an object and, upon first visit, enqueues it into an
/// internal queue inside the `ProcessEdgesWork` instance.  Each implementation of this trait
/// implement `trace_object` differently.  During [`Plan::schedule_collection`], plans select
/// (usually via `GCWorkContext`) specialized implementations of this trait to be used during each
/// trace according the nature of each trace, such as whether it is a nursery collection, whether it
/// is a defrag collection, whether it pins objects, etc.
///
/// This trait was originally designed for work packets that process object graph edges represented
/// as slots.  The constructor [`ProcessEdgesWork::new`] takes a vector of slots, and the created
/// work packet will trace the objects pointed by the object reference in each slot using the
/// `trace_object` method, and update the slot if the GC moves the target object when tracing.
///
/// This trait can also be used merely as a provider of the `trace_object` method by giving it an
/// empty vector of slots.  This is useful for node-enqueuing tracing
/// ([`Scanning::scan_object_and_trace_edges`]) as well as weak reference processing
/// ([`Scanning::process_weak_refs`] as well as `ReferenceProcessor` and `FinalizableProcessor`).
/// In those cases, the caller passes the reference to the target object to `trace_object`, an the
/// caller is responsible for updating the slots according the return value of `trace_object`.
///
/// TODO: We should refactor this trait to decouple it from slots. See:
/// <https://github.com/mmtk/mmtk-core/issues/599>

/// A general implementation of [`ProcessEdgesWork`] using SFT. A plan can always implement their
/// own [`ProcessEdgesWork`] instances. However, most plans can use this work packet for tracing amd
/// they do not need to provide a plan-specific trace object work packet. If they choose to use this
/// type, they need to provide a correct implementation for some related methods (such as
/// `Space.set_copy_for_sft_trace()`, `SFT.sft_trace_object()`). Some plans are not using this type,
/// mostly due to more complex tracing. Either it is impossible to use this type, or there is
/// performance overheads for using this general trace type. In such cases, they implement their
/// specific [`ProcessEdgesWork`] instances.
// TODO: This is not used any more. Should we remove it?
#[allow(dead_code)]

/// For USDT tracepoints for roots.
/// Keep in sync with `tools/tracing/timeline/visualize.py`.
#[repr(usize)]
enum RootsKind {
    NORMAL = 0,
    PINNING = 1,
    TPINNING = 2,
}

/// Trait for a work packet that scans objects

/// Generic slot-processing work packet parameterized by [`TracePolicy`].
///
/// This replaces plan-specific `ProcessEdgesWork` implementations.
/// All plans share this single work packet type — the plan-specific behavior
/// is provided by the `T: TracePolicy<VM>` parameter.
///
/// ```text
///   VM roots / ObjectsClosure
///         │
///         ▼
///   GCProcessEdges<VM, T>    (this type)
///   ├── load slot → trace_object (via T)
///   ├── buffer traced nodes
///   └── flush → GCScanObjects<VM, T>
/// ```
pub(crate) struct GCProcessEdges<VM: VMBinding, T: TracePolicy<VM>> {
    /// The slots to process.
    slots: Vec<VM::VMSlot>,
    /// Traced objects waiting to be scanned.
    nodes: VectorObjectQueue,
    /// Whether the slots come from root scanning.
    roots: bool,
    /// Reference to the MMTK instance.
    mmtk: &'static MMTK<VM>,
    /// The trace policy (plan-specific tracing behavior).
    policy: T,
    /// Which work bucket this packet belongs to.
    bucket: WorkBucketStage,
}

impl<VM: VMBinding, T: TracePolicy<VM>> GCProcessEdges<VM, T> {
    pub fn new(
        slots: Vec<VM::VMSlot>,
        roots: bool,
        mmtk: &'static MMTK<VM>,
        bucket: WorkBucketStage,
    ) -> Self {
        let policy = T::from_mmtk(mmtk);
        Self {
            slots,
            nodes: VectorObjectQueue::new(),
            roots,
            mmtk,
            policy,
            bucket,
        }
    }

    /// Process all slots using the given worker reference.
    fn process_slots_with_worker(&mut self, worker: &mut GCWorker<VM>) {
        probe!(mmtk, process_slots, self.slots.len(), self.roots);
        let may_move = self.policy.may_move_objects();
        for i in 0..self.slots.len() {
            let slot = self.slots[i];
            let Some(object) = slot.load() else {
                continue;
            };
            let new_object = self.policy.trace_object(&mut self.nodes, object, worker);
            if may_move && new_object != object {
                slot.store(new_object);
            }
        }
    }

    /// Flush the node buffer by creating a GCScanObjects work packet.
    fn flush_nodes(&mut self, worker: &mut GCWorker<VM>) {
        let nodes = self.nodes.take();
        if !nodes.is_empty() {
            let policy = self.policy.clone();
            let mut scan_work = GCScanObjects::<VM, T>::new(policy, nodes, false, self.bucket);
            // Execute immediately for better locality.
            scan_work.do_work_inner(worker, self.mmtk);
        }
    }
}

impl<VM: VMBinding, T: TracePolicy<VM>> GCWork<VM> for GCProcessEdges<VM, T> {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, _mmtk: &'static MMTK<VM>) {
        self.process_slots_with_worker(worker);
        if !self.nodes.is_empty() {
            self.flush_nodes(worker);
        }
        #[cfg(feature = "sanity")]
        if self.roots && !_mmtk.is_in_sanity() {
            _mmtk
                .sanity_checker
                .lock()
                .unwrap()
                .add_root_slots(self.slots.clone());
        }
        trace!("GCProcessEdges End");
    }
}

/// Generic scan-objects work packet parameterized by [`TracePolicy`].
///
/// This replaces `ScanObjects` and `PlanScanObjects`. It handles both
/// slot-enqueuing scanning (via `ObjectsClosure`) and node-enqueuing scanning
/// (via `GCTracerContext`).
///
/// ```text
///   GCProcessEdges (flush)
///         │
///         ▼
///   GCScanObjects<VM, T>     (this type)
///   ├── scan_object (slot-enqueuing) → ObjectsClosure → new GCProcessEdges
///   ├── scan_object_and_trace_edges (node-enqueuing) → GCTracerContext
///   └── post_scan_object (via T)
/// ```
pub(crate) struct GCScanObjects<VM: VMBinding, T: TracePolicy<VM>> {
    /// The trace policy.
    policy: T,
    /// Objects to scan.
    buffer: Vec<ObjectReference>,
    /// Whether these are root objects.
    roots: bool,
    /// Which work bucket this packet belongs to.
    bucket: WorkBucketStage,
    _phantom: PhantomData<VM>,
}

impl<VM: VMBinding, T: TracePolicy<VM>> GCScanObjects<VM, T> {
    pub fn new(
        policy: T,
        buffer: Vec<ObjectReference>,
        roots: bool,
        bucket: WorkBucketStage,
    ) -> Self {
        Self {
            policy,
            buffer,
            roots,
            bucket,
            _phantom: PhantomData,
        }
    }

    /// Inner work method, callable directly (for inline scan optimization).
    pub(crate) fn do_work_inner(&mut self, worker: &mut GCWorker<VM>, mmtk: &'static MMTK<VM>) {
        let tls = worker.tls;

        // Scan objects that support slot-enqueuing. Postpone others.
        let mut scan_later = vec![];
        {
            let mut closure = GCObjectsClosure::<VM, T>::new(worker, self.bucket, mmtk);

            // Count live bytes if enabled.
            if crate::util::rust_util::unlikely(*mmtk.get_options().count_live_bytes_in_gc) {
                let mut live_bytes_stats = closure.worker.shared.live_bytes_per_space.borrow_mut();
                for object in self.buffer.iter().copied() {
                    crate::scheduler::worker::GCWorkerShared::<VM>::increase_live_bytes(
                        &mut live_bytes_stats,
                        object,
                    );
                }
            }

            for object in self.buffer.iter().copied() {
                if <VM as VMBinding>::VMScanning::support_slot_enqueuing(tls, object) {
                    trace!("Scan object (slot) {}", object);
                    <VM as VMBinding>::VMScanning::scan_object(tls, object, &mut closure);
                    self.policy.post_scan_object(object);
                } else {
                    scan_later.push(object);
                }
            }
        }

        let total_objects = self.buffer.len();
        let scan_and_trace = scan_later.len();
        probe!(mmtk, scan_objects, total_objects, scan_and_trace);

        // For objects that don't support slot-enqueuing, use node-enqueuing.
        if !scan_later.is_empty() {
            let tracer_context = GCTracerContext::<VM, T> {
                stage: self.bucket,
                _phantom: PhantomData,
            };
            tracer_context.with_tracer(worker, |object_tracer| {
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

/// An `ObjectsClosure`-like slot visitor for the new TracePolicy-based system.
///
/// This buffers slots during object scanning and flushes them as new
/// `GCProcessEdges<VM, T>` work packets when the buffer is full.
struct GCObjectsClosure<'a, VM: VMBinding, T: TracePolicy<VM>> {
    buffer: VectorQueue<VM::VMSlot>,
    worker: &'a mut GCWorker<VM>,
    bucket: WorkBucketStage,
    mmtk: &'static MMTK<VM>,
    _phantom: PhantomData<T>,
}

impl<'a, VM: VMBinding, T: TracePolicy<VM>> GCObjectsClosure<'a, VM, T> {
    fn new(worker: &'a mut GCWorker<VM>, bucket: WorkBucketStage, mmtk: &'static MMTK<VM>) -> Self {
        Self {
            buffer: VectorQueue::new(),
            worker,
            bucket,
            mmtk,
            _phantom: PhantomData,
        }
    }

    fn flush(&mut self) {
        let buf = self.buffer.take();
        if !buf.is_empty() {
            self.worker.add_work(
                self.bucket,
                GCProcessEdges::<VM, T>::new(buf, false, self.mmtk, self.bucket),
            );
        }
    }
}

impl<VM: VMBinding, T: TracePolicy<VM>> SlotVisitor<VM::VMSlot> for GCObjectsClosure<'_, VM, T> {
    fn visit_slot(&mut self, slot: VM::VMSlot) {
        #[cfg(debug_assertions)]
        {
            use crate::vm::slot::Slot;
            trace!(
                "(GCObjectsClosure) Visit slot {:?} (pointing to {:?})",
                slot,
                slot.load()
            );
        }
        self.buffer.push(slot);
        if self.buffer.is_full() {
            self.flush();
        }
    }
}

impl<VM: VMBinding, T: TracePolicy<VM>> Drop for GCObjectsClosure<'_, VM, T> {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Wraps policy and node queue to implement [`ObjectTracer`].
///
/// Used for node-enqueuing scanning and weak reference processing,
/// where the scan callback traces objects directly instead of enqueuing slots.
pub(crate) struct GCTracer<VM: VMBinding, T: TracePolicy<VM>> {
    /// Traced objects waiting to be scanned.
    nodes: VectorObjectQueue,
    /// The trace policy.
    policy: T,
    /// Raw pointer to the current GC worker.  
    /// Set by `GCTracerContext::with_tracer` before the closure is called.
    /// # Safety  
    /// Valid only during the `with_tracer` closure. The pointer must not
    /// outlive the `GCWorker` reference it was derived from.
    worker: *mut GCWorker<VM>,
    /// Reference to MMTK instance, for creating work packets on drop.
    mmtk: &'static MMTK<VM>,
    /// Which work bucket to add scan work to.
    stage: WorkBucketStage,
}

impl<VM: VMBinding, T: TracePolicy<VM>> ObjectTracer for GCTracer<VM, T> {
    /// Forward the `trace_object` call to the policy.
    fn trace_object(&mut self, object: ObjectReference) -> ObjectReference {
        let worker: &mut GCWorker<VM> = unsafe { &mut *self.worker };
        self.policy.trace_object(&mut self.nodes, object, worker)
    }
}

impl<VM: VMBinding, T: TracePolicy<VM>> Drop for GCTracer<VM, T> {
    fn drop(&mut self) {
        // When the tracer is dropped, flush any remaining traced nodes
        // as a new GCScanObjects work packet scheduled via the scheduler.
        let nodes = self.nodes.take();
        if !nodes.is_empty() {
            let policy = self.policy.clone();
            let bucket = self.stage;
            crate::memory_manager::add_work_packet(
                self.mmtk,
                bucket,
                GCScanObjects::<VM, T>::new(policy, nodes, false, bucket),
            );
        }
    }
}

/// [`ObjectTracerContext`] implementation for the TracePolicy-based system.
///
/// This creates a temporary [`GCTracer`],
/// providing the `ObjectTracer` interface needed by weak reference processing,
/// finalizable processing, and node-enqueuing scanning.
pub(crate) struct GCTracerContext<VM: VMBinding, T: TracePolicy<VM>> {
    pub stage: WorkBucketStage,
    pub _phantom: PhantomData<(VM, T)>,
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
        let policy = T::from_mmtk(mmtk);

        let mut tracer = GCTracer {
            nodes: VectorObjectQueue::new(),
            policy,
            worker: worker as *mut GCWorker<VM>,
            mmtk,
            stage: self.stage,
        };

        let result = func(&mut tracer);

        // The tracer's Drop impl will flush remaining nodes.
        drop(tracer);

        result
    }
}

// ============================================================================
// Root scanning: GCRootsWorkFactory + GCProcessRootNodes
// ============================================================================

use crate::vm::RootsWorkFactory;

/// A [`RootsWorkFactory`] implementation using [`TracePolicy`] types.
///
/// This replaces `ProcessEdgesWorkRootsWorkFactory`. Instead of parameterizing
/// by `ProcessEdgesWork` types, it uses `TracePolicy` types from `GCWorkContext`.
///
/// - `DT`: Default trace policy (for normal roots)
/// - `PT`: Pinning trace policy (for pinning/transitive-pinning roots)
pub(crate) struct GCRootsWorkFactory<VM: VMBinding, DT: TracePolicy<VM>, PT: TracePolicy<VM>> {
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

impl<VM: VMBinding, DT: TracePolicy<VM>, PT: TracePolicy<VM>> GCRootsWorkFactory<VM, DT, PT> {
    pub fn new(mmtk: &'static MMTK<VM>) -> Self {
        Self {
            mmtk,
            _phantom: PhantomData,
        }
    }
}

impl<VM: VMBinding, DT: TracePolicy<VM>, PT: TracePolicy<VM>> RootsWorkFactory<VM::VMSlot>
    for GCRootsWorkFactory<VM, DT, PT>
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

/// Process root nodes using [`TracePolicy`].
///
/// Replaces the old `ProcessRootNodes` type. Root nodes (as opposed to root slots)
/// are pre-traced objects that should not move. After tracing, their children are
/// scanned using a potentially different trace policy (O2OT).
///
/// - `R2OT`: Root-to-object trace policy (traces the root objects themselves — must not move)
/// - `O2OT`: Object-to-object trace policy (traces the children of root objects)
pub(crate) struct GCProcessRootNodes<VM: VMBinding, R2OT: TracePolicy<VM>, O2OT: TracePolicy<VM>> {
    roots: Vec<ObjectReference>,
    bucket: WorkBucketStage,
    _phantom: PhantomData<(VM, R2OT, O2OT)>,
}

impl<VM: VMBinding, R2OT: TracePolicy<VM>, O2OT: TracePolicy<VM>>
    GCProcessRootNodes<VM, R2OT, O2OT>
{
    pub fn new(nodes: Vec<ObjectReference>, bucket: WorkBucketStage) -> Self {
        Self {
            roots: nodes,
            bucket,
            _phantom: PhantomData,
        }
    }
}

impl<VM: VMBinding, R2OT: TracePolicy<VM>, O2OT: TracePolicy<VM>> GCWork<VM>
    for GCProcessRootNodes<VM, R2OT, O2OT>
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

        // Trace root objects using the pinning trace policy.
        // These objects must not move (the VM can't update the root slots).
        let root_objects_to_scan = {
            let policy = R2OT::from_mmtk(mmtk);
            let mut nodes = VectorObjectQueue::new();

            for object in self.roots.iter().copied() {
                let new_object = policy.trace_object(&mut nodes, object, worker);
                debug_assert_eq!(
                    object, new_object,
                    "Object moved while tracing unmovable root object: {} -> {}",
                    object, new_object
                );
            }

            nodes.take()
        };

        let num_enqueued_nodes = root_objects_to_scan.len();
        probe!(mmtk, process_root_nodes, num_roots, num_enqueued_nodes);

        // Scan the traced root objects using the descendant trace policy.
        if !root_objects_to_scan.is_empty() {
            let policy = O2OT::from_mmtk(mmtk);
            let mut scan_work =
                GCScanObjects::<VM, O2OT>::new(policy, root_objects_to_scan, false, self.bucket);
            scan_work.do_work_inner(worker, mmtk);
        }

        trace!("GCProcessRootNodes End");
    }
}
