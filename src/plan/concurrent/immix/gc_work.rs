use crate::plan::concurrent::immix::global::ConcurrentImmix;
use crate::plan::{MatureTracePolicy, UnsupportedTracePolicy};
use crate::policy::gc_work::{TraceKind, TRACE_KIND_TRANSITIVE_PIN};

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
/// During concurrent marking pauses, root scanning is stop-the-world so standard
/// MatureTracePolicy works correctly for tracing root objects. The concurrent
/// marking phase itself uses its own `ConcurrentTraceObjects` work packets directly
/// (not through this context).
pub(super) struct ConcurrentImmixConcurrentGCWorkContext<VM: VMBinding>(
    std::marker::PhantomData<VM>,
);

impl<VM: VMBinding> crate::scheduler::GCWorkContext for ConcurrentImmixConcurrentGCWorkContext<VM> {
    type VM = VM;
    type PlanType = ConcurrentImmix<VM>;
    // Use the fast marking policy for root scanning during concurrent marking pauses.
    type DefaultTracePolicy =
        MatureTracePolicy<VM, ConcurrentImmix<VM>, { crate::policy::gc_work::DEFAULT_TRACE }>;
    type PinningTracePolicy = UnsupportedTracePolicy<VM>;
}
