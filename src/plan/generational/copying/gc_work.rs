use super::global::GenCopy;
use crate::plan::{MatureTracePolicy, NurseryTracePolicy, UnsupportedTracePolicy};
use crate::vm::*;

use crate::policy::gc_work::DEFAULT_TRACE;

pub struct GenCopyNurseryGCWorkContext<VM: VMBinding>(std::marker::PhantomData<VM>);
impl<VM: VMBinding> crate::scheduler::GCWorkContext for GenCopyNurseryGCWorkContext<VM> {
    type VM = VM;
    type PlanType = GenCopy<VM>;
    type DefaultTracePolicy = NurseryTracePolicy<VM, GenCopy<VM>, DEFAULT_TRACE>;
    type PinningTracePolicy = UnsupportedTracePolicy<VM>;
}

pub struct GenCopyGCWorkContext<VM: VMBinding>(std::marker::PhantomData<VM>);
impl<VM: VMBinding> crate::scheduler::GCWorkContext for GenCopyGCWorkContext<VM> {
    type VM = VM;
    type PlanType = GenCopy<VM>;
    type DefaultTracePolicy = MatureTracePolicy<VM, GenCopy<VM>, DEFAULT_TRACE>;
    type PinningTracePolicy = UnsupportedTracePolicy<VM>;
}
