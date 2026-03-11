use super::global::GenImmix;
use crate::plan::{MatureTracePolicy, NurseryTracePolicy, UnsupportedTracePolicy};
use crate::policy::gc_work::{DefaultTrace, TraceKind};
use crate::vm::VMBinding;

pub struct GenImmixNurseryGCWorkContext<VM: VMBinding>(std::marker::PhantomData<VM>);
impl<VM: VMBinding> crate::scheduler::GCWorkContext for GenImmixNurseryGCWorkContext<VM> {
    type VM = VM;
    type PlanType = GenImmix<VM>;
    type DefaultTracePolicy = NurseryTracePolicy<VM, GenImmix<VM>, DefaultTrace>;
    type PinningTracePolicy = UnsupportedTracePolicy<VM>;
}

pub(super) struct GenImmixMatureGCWorkContext<VM: VMBinding, K: TraceKind>(
    std::marker::PhantomData<(VM, K)>,
);
impl<VM: VMBinding, K: TraceKind> crate::scheduler::GCWorkContext
    for GenImmixMatureGCWorkContext<VM, K>
{
    type VM = VM;
    type PlanType = GenImmix<VM>;
    type DefaultTracePolicy = MatureTracePolicy<VM, GenImmix<VM>, K>;
    type PinningTracePolicy = UnsupportedTracePolicy<VM>;
}
