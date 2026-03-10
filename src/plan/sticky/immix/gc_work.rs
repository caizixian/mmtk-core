use crate::plan::{MatureTracePolicy, NurseryTracePolicy};
use crate::policy::gc_work::TraceKind;
use crate::policy::gc_work::DEFAULT_TRACE;
use crate::policy::gc_work::TRACE_KIND_TRANSITIVE_PIN;
use crate::vm::VMBinding;

use super::global::StickyImmix;

pub struct StickyImmixNurseryGCWorkContext<VM: VMBinding>(std::marker::PhantomData<VM>);

impl<VM: VMBinding> crate::scheduler::GCWorkContext for StickyImmixNurseryGCWorkContext<VM> {
    type VM = VM;
    type PlanType = StickyImmix<VM>;
    type DefaultTracePolicy = NurseryTracePolicy<VM, StickyImmix<VM>, DEFAULT_TRACE>;
    type PinningTracePolicy = NurseryTracePolicy<VM, StickyImmix<VM>, TRACE_KIND_TRANSITIVE_PIN>;
}

pub struct StickyImmixMatureGCWorkContext<VM: VMBinding, const KIND: TraceKind>(
    std::marker::PhantomData<VM>,
);
impl<VM: VMBinding, const KIND: TraceKind> crate::scheduler::GCWorkContext
    for StickyImmixMatureGCWorkContext<VM, KIND>
{
    type VM = VM;
    type PlanType = StickyImmix<VM>;
    type DefaultTracePolicy = MatureTracePolicy<VM, StickyImmix<VM>, KIND>;
    type PinningTracePolicy = MatureTracePolicy<VM, StickyImmix<VM>, TRACE_KIND_TRANSITIVE_PIN>;
}
