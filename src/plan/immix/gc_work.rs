use super::global::Immix;
use crate::plan::MatureTracePolicy;
use crate::policy::gc_work::{TraceKind, TransitivePinTrace};
use crate::vm::VMBinding;

pub(super) struct ImmixGCWorkContext<VM: VMBinding, K: TraceKind>(
    std::marker::PhantomData<(VM, K)>,
);
impl<VM: VMBinding, K: TraceKind> crate::scheduler::GCWorkContext
    for ImmixGCWorkContext<VM, K>
{
    type VM = VM;
    type PlanType = Immix<VM>;
    type DefaultTracePolicy = MatureTracePolicy<VM, Immix<VM>, K>;
    type PinningTracePolicy = MatureTracePolicy<VM, Immix<VM>, TransitivePinTrace>;
}
