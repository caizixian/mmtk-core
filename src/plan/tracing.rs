//! This module contains code useful for tracing,
//! i.e. visiting the reachable objects by traversing all or part of an object graph.

use std::marker::PhantomData;

use crate::scheduler::gc_work::ProcessEdgesWork;
use crate::scheduler::{GCWorker, WorkBucketStage, EDGES_WORK_BUFFER_SIZE};
use crate::util::{ObjectReference, VMThread, VMWorkerThread};
use crate::vm::slot::Slot;
use crate::vm::{Scanning, SlotVisitor, VMBinding};

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

/// Factory for creating slot-processing work packets.
///
/// This trait decouples the creation of slot-processing work packets from
/// [`ProcessEdgesWork::new`]. Any type implementing this trait can create work packets
/// that process slots (load references, trace objects, store back updated references).
///
/// A blanket implementation is provided for `PhantomData<E>` where `E: ProcessEdgesWork`,
/// so existing `ProcessEdgesWork` types can be used as factories without any changes.
pub trait SlotProcessorFactory<SL: Slot>: Send + 'static {
    type VM: VMBinding;

    /// Create a slot-processing work packet and add it to the given worker's queue.
    ///
    /// This method takes ownership of the slots and dispatches a work packet to process them.
    /// The implementation may use the worker's local work buffer for performance.
    fn add_slot_processing_work(
        &self,
        worker: &mut GCWorker<Self::VM>,
        slots: Vec<SL>,
        bucket: WorkBucketStage,
    );
}

/// Blanket impl: use `PhantomData<E>` as a factory for any `ProcessEdgesWork` type.
///
/// This creates work packets by calling `E::new(slots, false, mmtk, bucket)`.
impl<E: ProcessEdgesWork> SlotProcessorFactory<<E::VM as VMBinding>::VMSlot> for PhantomData<E> {
    type VM = E::VM;

    fn add_slot_processing_work(
        &self,
        worker: &mut GCWorker<Self::VM>,
        slots: Vec<<E::VM as VMBinding>::VMSlot>,
        bucket: WorkBucketStage,
    ) {
        worker.add_work(bucket, E::new(slots, false, worker.mmtk, bucket));
    }
}

/// A transitive closure visitor to collect the slots from objects.
/// It maintains a buffer for the slots, and flushes slots to a new work packet
/// if the buffer is full or if the type gets dropped.
///
/// This type is generic over a [`SlotProcessorFactory`] which determines how
/// the collected slots are packaged into work packets.
pub struct ObjectsClosure<'a, VM: VMBinding, F: SlotProcessorFactory<VM::VMSlot, VM = VM>> {
    buffer: VectorQueue<VM::VMSlot>,
    pub(crate) worker: &'a mut GCWorker<VM>,
    bucket: WorkBucketStage,
    factory: F,
}

impl<'a, VM: VMBinding, F: SlotProcessorFactory<VM::VMSlot, VM = VM>>
    ObjectsClosure<'a, VM, F>
{
    /// Create an [`ObjectsClosure`].
    ///
    /// Arguments:
    /// * `worker`: the current worker. The objects closure should not leave the context of this worker.
    /// * `bucket`: new work generated will be pushed to the bucket.
    /// * `factory`: the factory used to create slot-processing work packets.
    pub fn new(worker: &'a mut GCWorker<VM>, bucket: WorkBucketStage, factory: F) -> Self {
        Self {
            buffer: VectorQueue::new(),
            worker,
            bucket,
            factory,
        }
    }

    fn flush(&mut self) {
        let buf = self.buffer.take();
        if !buf.is_empty() {
            self.factory
                .add_slot_processing_work(self.worker, buf, self.bucket);
        }
    }
}

impl<VM: VMBinding, F: SlotProcessorFactory<VM::VMSlot, VM = VM>>
    SlotVisitor<VM::VMSlot> for ObjectsClosure<'_, VM, F>
{
    fn visit_slot(&mut self, slot: VM::VMSlot) {
        #[cfg(debug_assertions)]
        {
            use crate::vm::slot::Slot;
            trace!(
                "(ObjectsClosure) Visit slot {:?} (pointing to {:?})",
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

impl<VM: VMBinding, F: SlotProcessorFactory<VM::VMSlot, VM = VM>>
    Drop for ObjectsClosure<'_, VM, F>
{
    fn drop(&mut self) {
        self.flush();
    }
}

/// Type alias for backward compatibility: an `ObjectsClosure` using a `ProcessEdgesWork` type.
pub type ProcessEdgesObjectsClosure<'a, E> = ObjectsClosure<
    'a,
    <E as ProcessEdgesWork>::VM,
    PhantomData<E>,
>;

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
