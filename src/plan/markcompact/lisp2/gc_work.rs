use super::global::Lisp2;
use crate::plan::tracing::{PlanTrace, UnsupportedTrace};
use crate::policy::lisp2space::Lisp2Space;
use crate::policy::lisp2space::{TRACE_KIND_FORWARD, TRACE_KIND_MARK};
use crate::scheduler::gc_work::*;
use crate::scheduler::GCWork;
use crate::scheduler::GCWorker;
use crate::scheduler::WorkBucketStage;
use crate::vm::ActivePlan;
use crate::vm::Scanning;
use crate::vm::VMBinding;
use crate::MMTK;
use std::marker::PhantomData;

/// iterate through the heap and calculate the new location of live objects
pub struct CalculateForwardingAddress<VM: VMBinding> {
    space: &'static Lisp2Space<VM>,
}

impl<VM: VMBinding> GCWork<VM> for CalculateForwardingAddress<VM> {
    fn do_work(&mut self, _worker: &mut GCWorker<VM>, _mmtk: &'static MMTK<VM>) {
        self.space.calculate_forwarding_pointer();
    }
}

impl<VM: VMBinding> CalculateForwardingAddress<VM> {
    pub fn new(space: &'static Lisp2Space<VM>) -> Self {
        Self { space }
    }
}

/// create another round of root scanning work packets
/// to update object references
pub struct UpdateReferences<VM: VMBinding> {
    p: PhantomData<fn() -> VM>,
}

impl<VM: VMBinding> GCWork<VM> for UpdateReferences<VM> {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, mmtk: &'static MMTK<VM>) {
        // The following needs to be done right before the second round of root scanning
        VM::VMScanning::prepare_for_roots_re_scanning();
        mmtk.state.prepare_for_stack_scanning();
        // Prepare common and base spaces for the 2nd round of transitive closure
        {
            // SAFETY: `UpdateReferences` is scheduled as the sole initial packet in
            // `WorkBucketStage::SecondRoots` (which opens sequentially only after
            // `CalculateForwarding` drains and all other GC workers are parked, while mutators
            // remain stopped), before any `ScanMutatorRoots`/`ScanVMSpecificRoots` packets are
            // added below. No concurrent or re-entrant read/write access to `mmtk.plan` occurs
            // during this block, and `mmtk.get_plan_mut()` derives `&mut` from `MMTK::plan`'s
            // `Box<UnsafeCell<dyn Plan>>`.
            let plan_mut = unsafe { mmtk.get_plan_mut() }
                .downcast_mut::<Lisp2<VM>>()
                .unwrap();
            plan_mut.common.release(worker.tls, true);
            plan_mut.common.prepare(worker.tls, true);
        }
        #[cfg(feature = "extreme_assertions")]
        mmtk.slot_logger.reset();

        // We do two passes of transitive closures. We clear the live bytes from the first pass.
        mmtk.scheduler
            .worker_group
            .get_and_clear_worker_live_bytes();

        for mutator in VM::VMActivePlan::mutators() {
            mmtk.scheduler.work_buckets[WorkBucketStage::SecondRoots].add(ScanMutatorRoots::<
                Lisp2ForwardingGCWorkContext<VM>,
            >(mutator));
        }

        mmtk.scheduler.work_buckets[WorkBucketStage::SecondRoots]
            .add(ScanVMSpecificRoots::<Lisp2ForwardingGCWorkContext<VM>>::new());
    }
}

impl<VM: VMBinding> UpdateReferences<VM> {
    pub fn new(_plan: &Lisp2<VM>) -> Self {
        Self { p: PhantomData }
    }
}

/// compact live objects based on forwarding pointers calculated before
pub struct Compact<VM: VMBinding> {
    space: &'static Lisp2Space<VM>,
}

impl<VM: VMBinding> GCWork<VM> for Compact<VM> {
    fn do_work(&mut self, _worker: &mut GCWorker<VM>, _mmtk: &'static MMTK<VM>) {
        self.space.compact();
    }
}

impl<VM: VMBinding> Compact<VM> {
    pub fn new(space: &'static Lisp2Space<VM>) -> Self {
        Self { space }
    }
}

/// Marking trace
pub type MarkingTrace<VM> = PlanTrace<Lisp2<VM>, TRACE_KIND_MARK>;
/// Forwarding trace
pub type ForwardingTrace<VM> = PlanTrace<Lisp2<VM>, TRACE_KIND_FORWARD>;

pub struct Lisp2GCWorkContext<VM: VMBinding>(std::marker::PhantomData<VM>);
impl<VM: VMBinding> crate::scheduler::GCWorkContext for Lisp2GCWorkContext<VM> {
    type VM = VM;
    type PlanType = Lisp2<VM>;
    type DefaultTrace = MarkingTrace<VM>;
    type PinningTrace = UnsupportedTrace<VM>;
}

pub struct Lisp2ForwardingGCWorkContext<VM: VMBinding>(std::marker::PhantomData<VM>);
impl<VM: VMBinding> crate::scheduler::GCWorkContext for Lisp2ForwardingGCWorkContext<VM> {
    type VM = VM;
    type PlanType = Lisp2<VM>;
    type DefaultTrace = ForwardingTrace<VM>;
    type PinningTrace = UnsupportedTrace<VM>;
}
