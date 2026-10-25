#![deny(unsafe_op_in_unsafe_fn)]

use super::global::Lisp2;
use crate::plan::tracing::{PlanTrace, UnsupportedTrace};
use crate::plan::Plan;
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

/// Create another round of root scanning work packets to update object references.
///
/// Constructing `UpdateReferences::<VM>::new(plan)` registers a pre-open hook on
/// [`WorkBucketStage::SecondRoots`] so `common.{release, prepare}` runs inside `WorkerMonitor`
/// while all workers are parked right before `WorkBucketStage::SecondRoots` opens.
pub struct UpdateReferences<VM: VMBinding> {
    p: PhantomData<fn() -> VM>,
}

impl<VM: VMBinding> GCWork<VM> for UpdateReferences<VM> {
    fn do_work(&mut self, _worker: &mut GCWorker<VM>, _mmtk: &'static MMTK<VM>) {}
}

impl<VM: VMBinding> UpdateReferences<VM> {
    pub fn new(plan: &Lisp2<VM>) -> Self {
        plan.base().scheduler.work_buckets[WorkBucketStage::SecondRoots].set_pre_open_hook(
            |mmtk, tls, parked| {
                // The following needs to be done right before the second round of root scanning
                VM::VMScanning::prepare_for_roots_re_scanning();
                mmtk.state.prepare_for_stack_scanning();
                // Prepare common and base spaces for the 2nd round of transitive closure
                {
                    assert!(
                        parked.monitor_eq(&mmtk.scheduler.worker_monitor),
                        "WorkersParked belongs to a different MMTK instance"
                    );
                    assert_eq!(
                        mmtk.get_gc_status(),
                        crate::global_state::GcStatus::InPause,
                        "Lisp2 UpdateReferences requires GcStatus::InPause"
                    );
                    let _plan_lease = mmtk.enter_plan_mut_lease();
                    // SAFETY:
                    // 1. `parked` witnesses `mmtk.scheduler.worker_monitor` with
                    //    `WorkerMonitor::sync` held and all `0..worker_count` GC workers parked,
                    //    and `mmtk.get_gc_status() == GcStatus::InPause` guarantees all mutators
                    //    are stopped at GC safepoints.
                    // 2. `prepare_for_roots_re_scanning` ran above before acquiring `plan_mut`,
                    //    `_plan_lease` enforces non-reentrancy on this thread,
                    //    `common.{release, prepare}` performs only internal space updates without
                    //    calling `mmtk.get_plan()`, and `plan_mut` is dropped at the end of this
                    //    block.
                    let plan_mut = unsafe { mmtk.get_plan_mut() }
                        .downcast_mut::<Lisp2<VM>>()
                        .unwrap();
                    plan_mut.common.release(tls, true);
                    plan_mut.common.prepare(tls, true);
                }
                #[cfg(feature = "extreme_assertions")]
                mmtk.slot_logger.reset();

                // We do two passes of transitive closures. We clear the live bytes from the first pass.
                mmtk.scheduler
                    .worker_group
                    .get_and_clear_worker_live_bytes();

                let packets = VM::VMActivePlan::mutators()
                    .map(|mutator| {
                        Box::new(ScanMutatorRoots::<Lisp2ForwardingGCWorkContext<VM>>::new(
                            mutator,
                        )) as _
                    })
                    .collect::<Vec<_>>();
                mmtk.scheduler.work_buckets[WorkBucketStage::SecondRoots].bulk_add(packets);

                mmtk.scheduler.work_buckets[WorkBucketStage::SecondRoots]
                    .add(ScanVMSpecificRoots::<Lisp2ForwardingGCWorkContext<VM>>::new());
            },
        );
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
