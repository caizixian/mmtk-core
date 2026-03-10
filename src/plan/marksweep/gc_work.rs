use super::MarkSweep;
use crate::plan::MatureTracePolicy;
use crate::policy::gc_work::DEFAULT_TRACE;
use crate::vm::VMBinding;

pub struct MSGCWorkContext<VM: VMBinding>(std::marker::PhantomData<VM>);
impl<VM: VMBinding> crate::scheduler::GCWorkContext for MSGCWorkContext<VM> {
    type VM = VM;
    type PlanType = MarkSweep<VM>;
    type DefaultTracePolicy = MatureTracePolicy<VM, MarkSweep<VM>, DEFAULT_TRACE>;
    type PinningTracePolicy = MatureTracePolicy<VM, MarkSweep<VM>, DEFAULT_TRACE>;
}
