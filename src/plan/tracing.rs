//! This module contains code useful for tracing,
//! i.e. visiting the reachable objects by traversing all or part of an object graph.

use std::marker::PhantomData;

use crate::scheduler::{GCWorker, EDGES_WORK_BUFFER_SIZE};
use crate::util::{ObjectReference, VMThread, VMWorkerThread};
use crate::vm::{Scanning, VMBinding};

/// This trait represents an object queue to enqueue objects during tracing.
pub trait ObjectQueue {
    /// Enqueue an object into the queue.
    fn enqueue(&mut self, object: ObjectReference);
}

/// A vector queue for object references.
pub type VectorObjectQueue = VectorQueue<ObjectReference>;

/// An implementation of `ObjectQueue` using a `Vec`.
///
/// This can also be used as a buffer. For example, the mark stack or the write barrier mod-buffer.
pub struct VectorQueue<T> {
    /// Enqueued nodes.
    buffer: Vec<T>,
}

impl<T> VectorQueue<T> {
    /// Reserve a capacity of this on first enqueue to avoid frequent resizing.
    const CAPACITY: usize = EDGES_WORK_BUFFER_SIZE;

    /// Create an empty `VectorObjectQueue`.
    pub fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    /// Return `true` if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Return the contents of the underlying vector.  It will empty the queue.
    pub fn take(&mut self) -> Vec<T> {
        std::mem::take(&mut self.buffer)
    }

    /// Consume this `VectorObjectQueue` and return its underlying vector.
    pub fn into_vec(self) -> Vec<T> {
        self.buffer
    }

    /// Check if the buffer size reaches `CAPACITY`.
    pub fn is_full(&self) -> bool {
        self.buffer.len() >= Self::CAPACITY
    }

    /// Push an element to the queue. If the queue is empty, it will reserve
    /// space to hold the number of elements defined by the capacity.
    /// The user of this method needs to make sure the queue length does
    /// not exceed the capacity to avoid allocating more space
    /// (this method will not check the length against the capacity).
    pub fn push(&mut self, v: T) {
        if self.buffer.is_empty() {
            self.buffer.reserve(Self::CAPACITY);
        }
        self.buffer.push(v);
    }

    /// Return the len of the queue
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Empty the queue
    pub fn clear(&mut self) {
        self.buffer.clear()
    }
}

impl<T> Default for VectorQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl ObjectQueue for VectorQueue<ObjectReference> {
    fn enqueue(&mut self, v: ObjectReference) {
        self.push(v);
    }
}

// ============================================================================
// TracePolicy: plan-specific tracing abstraction
// ============================================================================
//
// Architecture overview:
//
//   ┌──────────────────────────────────────────────────────────────┐
//   │                    GC Heap Traversal                        │
//   │                                                              │
//   │  ┌─────────────────┐       ┌──────────────────┐             │
//   │  │ GCProcessEdges  │──────▶│  GCScanObjects   │             │
//   │  │ (process slots) │◀──────│  (scan objects)   │             │
//   │  └────────┬────────┘       └──────────────────┘             │
//   │           │                                                  │
//   │           │ delegates to                                     │
//   │           ▼                                                  │
//   │  ┌─────────────────┐                                        │
//   │  │  TracePolicy    │  ← ONLY plan-specific part             │
//   │  │  (trace_object) │                                        │
//   │  └────────┬────────┘                                        │
//   │           │                                                  │
//   │           ▼                                                  │
//   │  ┌─────────────────┐       ┌──────────────────┐             │
//   │  │PlanTraceObject  │──────▶│PolicyTraceObject  │             │
//   │  │ (per-plan)      │       │  (per-space)      │             │
//   │  └─────────────────┘       └──────────────────┘             │
//   └──────────────────────────────────────────────────────────────┘
//
// TracePolicy captures the ONLY plan-specific part: how to trace a single
// object. The generic work packets (GCProcessEdges, GCScanObjects) handle
// all other concerns: slot processing, node buffering, work packet lifecycle,
// and object scanning dispatch.
//
// Concrete policies:
//   - MatureTracePolicy<VM, P, K>       - standard trace via PlanTraceObject
//   - NurseryTracePolicy<VM, P, K>      - nursery trace for generational plans
//   - UnsupportedTracePolicy<VM>        - runtime panic placeholder

use crate::plan::generational::global::GenerationalPlanExt;
use crate::plan::global::PlanTraceObject;
use crate::plan::Plan;
use crate::policy::gc_work::TraceKind;
use crate::MMTK;

/// The plan-specific tracing policy. This is the **only** thing a plan needs
/// to provide for heap traversal.
///
/// A `TracePolicy` encapsulates how to trace a single object (i.e., mark or
/// copy it), and optionally what to do after scanning each object. All other
/// concerns — slot processing, node queue management, work packet lifecycle,
/// scan work creation — are handled generically by `GCProcessEdges` and
/// `GCScanObjects`.
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

/// A `TracePolicy` that traces objects using [`PlanTraceObject::trace_object`]
/// with a specific [`TraceKind`]. This is the standard trace policy used by
/// most plans for mature-space tracing.
pub struct MatureTracePolicy<
    VM: VMBinding,
    P: Plan<VM = VM> + PlanTraceObject<VM>,
    K: TraceKind,
> {
    plan: &'static P,
    _phantom: PhantomData<(VM, K)>,
}

impl<VM: VMBinding, P: Plan<VM = VM> + PlanTraceObject<VM>, K: TraceKind> Clone
    for MatureTracePolicy<VM, P, K>
{
    fn clone(&self) -> Self {
        Self {
            plan: self.plan,
            _phantom: PhantomData,
        }
    }
}

impl<VM: VMBinding, P: Plan<VM = VM> + PlanTraceObject<VM>, K: TraceKind> TracePolicy<VM>
    for MatureTracePolicy<VM, P, K>
{
    fn may_move_objects(&self) -> bool {
        self.plan.may_move_objects::<K>()
    }

    fn trace_object(
        &self,
        queue: &mut VectorObjectQueue,
        object: ObjectReference,
        worker: &mut GCWorker<VM>,
    ) -> ObjectReference {
        self.plan
            .trace_object::<VectorObjectQueue, K>(queue, object, worker)
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

/// A `TracePolicy` for nursery GC in generational plans. Traces objects using
/// [`GenerationalPlanExt::trace_object_nursery`].
pub struct NurseryTracePolicy<
    VM: VMBinding,
    P: GenerationalPlanExt<VM> + PlanTraceObject<VM>,
    K: TraceKind,
> {
    plan: &'static P,
    _phantom: PhantomData<(VM, K)>,
}

impl<VM: VMBinding, P: GenerationalPlanExt<VM> + PlanTraceObject<VM>, K: TraceKind> Clone
    for NurseryTracePolicy<VM, P, K>
{
    fn clone(&self) -> Self {
        Self {
            plan: self.plan,
            _phantom: PhantomData,
        }
    }
}

impl<VM: VMBinding, P: GenerationalPlanExt<VM> + PlanTraceObject<VM>, K: TraceKind>
    TracePolicy<VM> for NurseryTracePolicy<VM, P, K>
{
    fn may_move_objects(&self) -> bool {
        true
    }

    fn trace_object(
        &self,
        queue: &mut VectorObjectQueue,
        object: ObjectReference,
        worker: &mut GCWorker<VM>,
    ) -> ObjectReference {
        self.plan
            .trace_object_nursery::<VectorObjectQueue, K>(queue, object, worker)
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
/// certain root kinds (e.g., pinning or transitive pinning roots).
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
        unreachable!("UnsupportedTracePolicy: this plan does not support this trace kind")
    }

    fn trace_object(
        &self,
        _queue: &mut VectorObjectQueue,
        _object: ObjectReference,
        _worker: &mut GCWorker<VM>,
    ) -> ObjectReference {
        unreachable!("UnsupportedTracePolicy: this plan does not support this trace kind")
    }

    fn from_mmtk(_mmtk: &'static MMTK<VM>) -> Self {
        unreachable!("UnsupportedTracePolicy: this plan does not support this trace kind")
    }
}

/// For iterating over the slots of an object.
// FIXME: This type iterates slots, but all of its current use cases only care about the values in the slots.
// And it currently only works if the object supports slot enqueuing (i.e. `Scanning::scan_object` is implemented).
// We may refactor the interface according to <https://github.com/mmtk/mmtk-core/issues/1375>
pub(crate) struct SlotIterator<VM: VMBinding> {
    _p: PhantomData<VM>,
}

impl<VM: VMBinding> SlotIterator<VM> {
    /// Iterate over the slots of an object by applying a function to each slot.
    pub fn iterate_fields<F: FnMut(VM::VMSlot)>(object: ObjectReference, _tls: VMThread, mut f: F) {
        // FIXME: We should use tls from the arguments.
        // See https://github.com/mmtk/mmtk-core/issues/1375
        let fake_tls = VMWorkerThread(VMThread::UNINITIALIZED);
        if !<VM::VMScanning as Scanning<VM>>::support_slot_enqueuing(fake_tls, object) {
            panic!("SlotIterator::iterate_fields cannot be used on objects that don't support slot-enqueuing");
        }
        <VM::VMScanning as Scanning<VM>>::scan_object(fake_tls, object, &mut f);
    }
}
