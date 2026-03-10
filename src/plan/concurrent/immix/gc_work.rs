use crate::plan::concurrent::immix::global::ConcurrentImmix;
use crate::plan::{MatureTracePolicy, UnsupportedTracePolicy};
use crate::policy::gc_work::{TraceKind, TRACE_KIND_TRANSITIVE_PIN};
use crate::scheduler::gc_work::{PlanProcessEdges, UnsupportedProcessEdges};
use crate::scheduler::ProcessEdgesWork;
use crate::vm::VMBinding;

pub(super) struct ConcurrentImmixSTWGCWorkContext<VM: VMBinding, const KIND: TraceKind>(
    std::marker::PhantomData<VM>,
);
impl<VM: VMBinding, const KIND: TraceKind> crate::scheduler::GCWorkContext
    for ConcurrentImmixSTWGCWorkContext<VM, KIND>
{
    type VM = VM;
    type PlanType = ConcurrentImmix<VM>;
    type DefaultProcessEdges = PlanProcessEdges<VM, ConcurrentImmix<VM>, KIND>;
    type PinningProcessEdges = PlanProcessEdges<VM, ConcurrentImmix<VM>, TRACE_KIND_TRANSITIVE_PIN>;
    type DefaultTracePolicy = MatureTracePolicy<VM, ConcurrentImmix<VM>, KIND>;
    type PinningTracePolicy = MatureTracePolicy<VM, ConcurrentImmix<VM>, TRACE_KIND_TRANSITIVE_PIN>;
}

/// GCWorkContext for the concurrent marking phase.
/// The DefaultProcessEdges is provided by the caller (e.g. ProcessRootSlots).
/// TracePolicy types are not used during concurrent marking — the concurrent
/// marking code uses its own `ConcurrentTraceObjects` work packets.
pub(super) struct ConcurrentImmixGCWorkContext<E: ProcessEdgesWork>(std::marker::PhantomData<E>);

impl<E: ProcessEdgesWork> crate::scheduler::GCWorkContext for ConcurrentImmixGCWorkContext<E> {
    type VM = E::VM;
    type PlanType = ConcurrentImmix<E::VM>;
    type DefaultProcessEdges = E;
    type PinningProcessEdges = UnsupportedProcessEdges<Self::VM>;
    // Concurrent marking doesn't use TracePolicy-based work packets.
    type DefaultTracePolicy = UnsupportedTracePolicy<E::VM>;
    type PinningTracePolicy = UnsupportedTracePolicy<E::VM>;
}
