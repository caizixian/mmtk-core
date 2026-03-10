use super::global::PageProtect;
use crate::policy::gc_work::DEFAULT_TRACE;
use crate::scheduler::gc_work::MatureTracePolicy;
use crate::vm::VMBinding;

pub struct PPGCWorkContext<VM: VMBinding>(std::marker::PhantomData<VM>);
impl<VM: VMBinding> crate::scheduler::GCWorkContext for PPGCWorkContext<VM> {
    type VM = VM;
    type PlanType = PageProtect<VM>;
    type DefaultTracePolicy = MatureTracePolicy<VM, PageProtect<VM>, DEFAULT_TRACE>;
    type PinningTracePolicy = MatureTracePolicy<VM, PageProtect<VM>, DEFAULT_TRACE>;
}
