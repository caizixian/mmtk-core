use crate::plan::ObjectQueue;
use crate::scheduler::GCWorker;
use crate::util::copy::CopySemantics;
use crate::util::ObjectReference;
use crate::vm::VMBinding;

// --- TraceKind: sealed marker trait + zero-sized types ---

mod sealed {
    pub trait Sealed {}
}

/// Marker trait for trace kinds. Implemented by zero-sized types that describe
/// trace behavior through associated boolean constants.
///
/// Policies check *properties* (`IS_DEFRAG`, `IS_FORWARD`, `IS_TRANSITIVE_PIN`)
/// rather than matching against specific trace kinds. This means each policy
/// only needs to handle the properties it cares about — Immix checks `IS_DEFRAG`,
/// MarkCompact checks `IS_FORWARD`, and neither needs to know about the other.
///
/// Because each trace kind is a distinct type, the Rust compiler monomorphizes
/// all call sites, eliminating dead branches at compile time — the same
/// zero-overhead specialization as the old `const KIND: u8` approach.
pub trait TraceKind: sealed::Sealed + Send + Copy + 'static {
    /// Whether this trace may defragment (opportunistically copy objects
    /// out of fragmented regions). Used by Immix.
    const IS_DEFRAG: bool = false;
    /// Whether this is a forwarding/reference-updating trace. Used by
    /// multi-phase compacting collectors (MarkCompact, Compressor).
    const IS_FORWARD: bool = false;
    /// Whether traced objects (and everything transitively reachable)
    /// must not be moved.
    const IS_TRANSITIVE_PIN: bool = false;
}

/// Default trace: mark in place, no movement.
#[derive(Clone, Copy)]
pub struct DefaultTrace;

/// Defrag trace: Immix opportunistic copying from fragmented blocks.
#[derive(Clone, Copy)]
pub struct DefragTrace;

/// Forwarding trace: update references (MarkCompact/Compressor phase 2).
#[derive(Clone, Copy)]
pub struct ForwardTrace;

/// Transitive pinning: pin object and all reachable descendants.
#[derive(Clone, Copy)]
pub struct TransitivePinTrace;

impl sealed::Sealed for DefaultTrace {}
impl sealed::Sealed for DefragTrace {}
impl sealed::Sealed for ForwardTrace {}
impl sealed::Sealed for TransitivePinTrace {}

impl TraceKind for DefaultTrace {}
impl TraceKind for DefragTrace {
    const IS_DEFRAG: bool = true;
}
impl TraceKind for ForwardTrace {
    const IS_FORWARD: bool = true;
}
impl TraceKind for TransitivePinTrace {
    const IS_TRANSITIVE_PIN: bool = true;
}

/// This trait defines policy-specific behavior for tracing objects.
/// The procedural macro `#[derive(PlanTraceObject)]` will generate code
/// that uses this trait. We expect any policy to implement this trait.
/// For the sake of performance, the implementation
/// of this trait should mark methods as `#[inline(always)]`.
pub trait PolicyTraceObject<VM: VMBinding> {
    /// Trace object in the policy. If the policy copies objects, we should
    /// expect `copy` to be a `Some` value.
    ///
    /// The type parameter `K` identifies the trace kind. Implementations should
    /// check properties like `K::IS_DEFRAG` or `K::IS_FORWARD` rather than
    /// matching against specific trace kind types.
    fn trace_object<Q: ObjectQueue, K: TraceKind>(
        &self,
        queue: &mut Q,
        object: ObjectReference,
        copy: Option<CopySemantics>,
        worker: &mut GCWorker<VM>,
    ) -> ObjectReference;

    /// Policy-specific post-scan-object hook.  It is called after scanning
    /// each object in this space.
    fn post_scan_object(&self, _object: ObjectReference) {
        // Do nothing.
    }

    /// Return whether this policy's space may move objects during the given
    /// trace kind.
    ///
    /// This is an instance method so that it can consult space-specific state
    /// (e.g., whether an Immix space is configured as non-moving).
    fn may_move_objects<K: TraceKind>(&self) -> bool;
}
