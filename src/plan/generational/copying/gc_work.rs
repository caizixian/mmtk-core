use super::global::GenCopy;
use crate::plan::generational::gc_work::GenNurseryProcessEdges;
use crate::vm::*;

use crate::policy::gc_work::DEFAULT_TRACE;
use crate::scheduler::gc_work::{MatureTracePolicy, NurseryTracePolicy, PlanProcessEdges, UnsupportedProcessEdges, UnsupportedTracePolicy};

pub struct GenCopyNurseryGCWorkContext<VM: VMBinding>(std::marker::PhantomData<VM>);
impl<VM: VMBinding> crate::scheduler::GCWorkContext for GenCopyNurseryGCWorkContext<VM> {
    type VM = VM;
    type PlanType = GenCopy<VM>;
    type DefaultProcessEdges = GenNurseryProcessEdges<Self::VM, Self::PlanType, DEFAULT_TRACE>;
    type PinningProcessEdges = UnsupportedProcessEdges<VM>;
    type DefaultTracePolicy = NurseryTracePolicy<VM, GenCopy<VM>, DEFAULT_TRACE>;
    type PinningTracePolicy = UnsupportedTracePolicy<VM>;
}

pub struct GenCopyGCWorkContext<VM: VMBinding>(std::marker::PhantomData<VM>);
impl<VM: VMBinding> crate::scheduler::GCWorkContext for GenCopyGCWorkContext<VM> {
    type VM = VM;
    type PlanType = GenCopy<VM>;
    type DefaultProcessEdges = PlanProcessEdges<Self::VM, GenCopy<VM>, DEFAULT_TRACE>;
    type PinningProcessEdges = UnsupportedProcessEdges<VM>;
    type DefaultTracePolicy = MatureTracePolicy<VM, GenCopy<VM>, DEFAULT_TRACE>;
    type PinningTracePolicy = UnsupportedTracePolicy<VM>;
}
