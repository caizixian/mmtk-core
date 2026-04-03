//! This module holds work packets for `CommonPlan` and `BasePlan`, or other work packets not
//! directly related to scheduling.

use crate::{scheduler::GCWork, vm::VMBinding};
use std::marker::PhantomData;

pub(super) struct SetCommonPlanUnlogBits<VM: VMBinding> {
    _phantom: PhantomData<VM>,
}

impl<VM: VMBinding> SetCommonPlanUnlogBits<VM> {
    pub fn new() -> Self {
        Self { _phantom: PhantomData }
    }
}

impl<VM: VMBinding> GCWork<VM> for SetCommonPlanUnlogBits<VM> {
    fn do_work(
        &mut self,
        _worker: &mut crate::scheduler::GCWorker<VM>,
        mmtk: &'static crate::MMTK<VM>,
    ) {
        mmtk.get_plan().common().set_side_log_bits();
    }
}

pub(super) struct ClearCommonPlanUnlogBits<VM: VMBinding> {
    _phantom: PhantomData<VM>,
}

impl<VM: VMBinding> ClearCommonPlanUnlogBits<VM> {
    pub fn new() -> Self {
        Self { _phantom: PhantomData }
    }
}

impl<VM: VMBinding> GCWork<VM> for ClearCommonPlanUnlogBits<VM> {
    fn do_work(
        &mut self,
        _worker: &mut crate::scheduler::GCWorker<VM>,
        mmtk: &'static crate::MMTK<VM>,
    ) {
        mmtk.get_plan().common().clear_side_log_bits();
    }
}
