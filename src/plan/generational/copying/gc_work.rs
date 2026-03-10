use super::global::GenCopy;
use crate::plan::generational::gc_work::GenNurseryTracePolicy;
use crate::vm::*;

use crate::policy::gc_work::DEFAULT_TRACE;
use crate::scheduler::gc_work::{PlanObjectTracePolicy, PolicyDrivenProcessEdges, UnsupportedProcessEdges};

pub struct GenCopyNurseryGCWorkContext<VM: VMBinding>(std::marker::PhantomData<VM>);
impl<VM: VMBinding> crate::scheduler::GCWorkContext for GenCopyNurseryGCWorkContext<VM> {
    type VM = VM;
    type PlanType = GenCopy<VM>;
    type DefaultProcessEdges =
        PolicyDrivenProcessEdges<Self::VM, GenNurseryTracePolicy<Self::VM, Self::PlanType, DEFAULT_TRACE>>;
    type PinningProcessEdges = UnsupportedProcessEdges<VM>;
}

pub struct GenCopyGCWorkContext<VM: VMBinding>(std::marker::PhantomData<VM>);
impl<VM: VMBinding> crate::scheduler::GCWorkContext for GenCopyGCWorkContext<VM> {
    type VM = VM;
    type PlanType = GenCopy<VM>;
    type DefaultProcessEdges =
        PolicyDrivenProcessEdges<Self::VM, PlanObjectTracePolicy<Self::VM, GenCopy<VM>, DEFAULT_TRACE>>;
    type PinningProcessEdges = UnsupportedProcessEdges<VM>;
}
