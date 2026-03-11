use crate::plan::{MatureTracePolicy, NurseryTracePolicy};
use crate::policy::gc_work::{DefaultTrace, TraceKind, TransitivePinTrace};
use crate::vm::VMBinding;

use super::global::StickyImmix;

pub struct StickyImmixNurseryGCWorkContext<VM: VMBinding>(std::marker::PhantomData<VM>);

impl<VM: VMBinding> crate::scheduler::GCWorkContext for StickyImmixNurseryGCWorkContext<VM> {
    type VM = VM;
    type PlanType = StickyImmix<VM>;
    type DefaultTracePolicy = NurseryTracePolicy<VM, StickyImmix<VM>, DefaultTrace>;
    type PinningTracePolicy = NurseryTracePolicy<VM, StickyImmix<VM>, TransitivePinTrace>;
}

pub struct StickyImmixMatureGCWorkContext<VM: VMBinding, K: TraceKind>(
    std::marker::PhantomData<(VM, K)>,
);
impl<VM: VMBinding, K: TraceKind> crate::scheduler::GCWorkContext
    for StickyImmixMatureGCWorkContext<VM, K>
{
    type VM = VM;
    type PlanType = StickyImmix<VM>;
    type DefaultTracePolicy = MatureTracePolicy<VM, StickyImmix<VM>, K>;
    type PinningTracePolicy = MatureTracePolicy<VM, StickyImmix<VM>, TransitivePinTrace>;
}
