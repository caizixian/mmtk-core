use crate::plan::concurrent::immix::global::ConcurrentImmix;
use crate::policy::gc_work::{TraceKind, TRACE_KIND_TRANSITIVE_PIN};
use crate::scheduler::gc_work::{PlanObjectTracePolicy, PolicyDrivenProcessEdges, UnsupportedProcessEdges};
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
    type DefaultProcessEdges =
        PolicyDrivenProcessEdges<VM, PlanObjectTracePolicy<VM, ConcurrentImmix<VM>, KIND>>;
    type PinningProcessEdges =
        PolicyDrivenProcessEdges<VM, PlanObjectTracePolicy<VM, ConcurrentImmix<VM>, TRACE_KIND_TRANSITIVE_PIN>>;
}

// ConcurrentImmixGCWorkContext is parameterized by a ProcessEdgesWork type (for concurrent marking).
// This cannot be migrated yet as it uses an arbitrary ProcessEdgesWork type.
pub(super) struct ConcurrentImmixGCWorkContext<E: ProcessEdgesWork>(std::marker::PhantomData<E>);

impl<E: ProcessEdgesWork> crate::scheduler::GCWorkContext for ConcurrentImmixGCWorkContext<E> {
    type VM = E::VM;
    type PlanType = ConcurrentImmix<E::VM>;
    type DefaultProcessEdges = E;
    type PinningProcessEdges = UnsupportedProcessEdges<Self::VM>;
}
