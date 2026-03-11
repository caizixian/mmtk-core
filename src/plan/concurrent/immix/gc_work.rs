use crate::plan::concurrent::immix::global::ConcurrentImmix;
use crate::plan::{MatureTracePolicy, UnsupportedTracePolicy};
use crate::policy::gc_work::{DefaultTrace, TraceKind, TransitivePinTrace};

use crate::vm::VMBinding;

pub(super) struct ConcurrentImmixSTWGCWorkContext<VM: VMBinding, K: TraceKind>(
    std::marker::PhantomData<(VM, K)>,
);
impl<VM: VMBinding, K: TraceKind> crate::scheduler::GCWorkContext
    for ConcurrentImmixSTWGCWorkContext<VM, K>
{
    type VM = VM;
    type PlanType = ConcurrentImmix<VM>;
    type DefaultTracePolicy = MatureTracePolicy<VM, ConcurrentImmix<VM>, K>;
    type PinningTracePolicy = MatureTracePolicy<VM, ConcurrentImmix<VM>, TransitivePinTrace>;
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
        MatureTracePolicy<VM, ConcurrentImmix<VM>, DefaultTrace>;
    type PinningTracePolicy = UnsupportedTracePolicy<VM>;
}
