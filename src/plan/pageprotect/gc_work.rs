use super::global::PageProtect;
use crate::plan::MatureTracePolicy;
use crate::policy::gc_work::DefaultTrace;
use crate::vm::VMBinding;

pub struct PPGCWorkContext<VM: VMBinding>(std::marker::PhantomData<VM>);
impl<VM: VMBinding> crate::scheduler::GCWorkContext for PPGCWorkContext<VM> {
    type VM = VM;
    type PlanType = PageProtect<VM>;
    type DefaultTracePolicy = MatureTracePolicy<VM, PageProtect<VM>, DefaultTrace>;
    type PinningTracePolicy = MatureTracePolicy<VM, PageProtect<VM>, DefaultTrace>;
}
