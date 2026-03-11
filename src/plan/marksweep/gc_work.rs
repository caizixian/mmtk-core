use super::MarkSweep;
use crate::plan::MatureTracePolicy;
use crate::policy::gc_work::DefaultTrace;
use crate::vm::VMBinding;

pub struct MSGCWorkContext<VM: VMBinding>(std::marker::PhantomData<VM>);
impl<VM: VMBinding> crate::scheduler::GCWorkContext for MSGCWorkContext<VM> {
    type VM = VM;
    type PlanType = MarkSweep<VM>;
    type DefaultTracePolicy = MatureTracePolicy<VM, MarkSweep<VM>, DefaultTrace>;
    type PinningTracePolicy = MatureTracePolicy<VM, MarkSweep<VM>, DefaultTrace>;
}
