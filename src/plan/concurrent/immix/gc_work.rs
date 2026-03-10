use crate::plan::concurrent::immix::global::ConcurrentImmix;
use crate::policy::gc_work::{TraceKind, TRACE_KIND_TRANSITIVE_PIN};
use crate::scheduler::gc_work::{MatureTracePolicy, UnsupportedTracePolicy};
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
    type DefaultTracePolicy = MatureTracePolicy<VM, ConcurrentImmix<VM>, KIND>;
    type PinningTracePolicy = MatureTracePolicy<VM, ConcurrentImmix<VM>, TRACE_KIND_TRANSITIVE_PIN>;
}

/// GCWorkContext for concurrent marking pauses.
/// During concurrent marking, root scanning is STW so MatureTracePolicy works correctly.
/// The concurrent marking phase itself uses its own ConcurrentTraceObjects directly.
pub(super) struct ConcurrentImmixGCWorkContext<E: ProcessEdgesWork>(std::marker::PhantomData<E>);

impl<E: ProcessEdgesWork> crate::scheduler::GCWorkContext for ConcurrentImmixGCWorkContext<E> {
    type VM = E::VM;
    type PlanType = ConcurrentImmix<E::VM>;
    // Use the fast marking policy for root scanning during concurrent marking pauses.
    // Root scanning is STW, so normal TracePolicy works correctly.
    type DefaultTracePolicy = MatureTracePolicy<E::VM, ConcurrentImmix<E::VM>, {crate::policy::gc_work::DEFAULT_TRACE}>;
    type PinningTracePolicy = UnsupportedTracePolicy<Self::VM>;
}
