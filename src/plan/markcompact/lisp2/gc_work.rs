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
use crate::scheduler::{SoloStageWork, SoloWorkerToken};
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
/// Constructing `UpdateReferences::<VM>::new(plan)` registers [`UpdateReferencesSolo`] as solo
/// stage work for [`WorkBucketStage::SecondRoots`] so `common.{release, prepare}` runs on a single
/// worker while all other workers stay gated in `WorkerMonitor` (even if `common.prepare` pushes
/// packets to already-open `WorkBucketStage::Prepare`).
pub struct UpdateReferences<VM: VMBinding> {
    p: PhantomData<fn() -> VM>,
}

impl<VM: VMBinding> GCWork<VM> for UpdateReferences<VM> {
    fn do_work(&mut self, _worker: &mut GCWorker<VM>, _mmtk: &'static MMTK<VM>) {}
}

impl<VM: VMBinding> UpdateReferences<VM> {
    pub fn new(plan: &Lisp2<VM>) -> Self {
        plan.base().scheduler.set_solo_stage_work(
            WorkBucketStage::SecondRoots,
            UpdateReferencesSolo::<VM> { p: PhantomData },
        );
        Self { p: PhantomData }
    }
}

struct UpdateReferencesSolo<VM: VMBinding> {
    p: PhantomData<fn() -> VM>,
}

impl<VM: VMBinding> SoloStageWork<VM> for UpdateReferencesSolo<VM> {
    fn do_solo_work(
        &mut self,
        worker: &mut GCWorker<VM>,
        mmtk: &'static MMTK<VM>,
        solo: SoloWorkerToken<'_>,
    ) {
        assert_eq!(solo.instance_id(), mmtk.id);
        // The following needs to be done right before the second round of root scanning
        VM::VMScanning::prepare_for_roots_re_scanning();
        mmtk.state.prepare_for_stack_scanning();
        // Prepare common and base spaces for the 2nd round of transitive closure
        {
            let _plan_lease = mmtk.enter_plan_mut_lease();
            // SAFETY:
            // 1. `solo: SoloWorkerToken<'_>` (`solo.instance_id() == mmtk.id`) is minted solely by
            //    `WorkerMonitor::enter_solo_packet` after `all_parked` locked all other
            //    `worker_count - 1` `WorkerPerm`s of `mmtk.id` inside `WorkerMonitor::park_and_wait`
            //    (gated across spurious Condvar wakeups, with `notify_work_available` calls from
            //    `common.prepare` pushing into already-open `WorkBucketStage::Prepare` deferred
            //    until `SoloExecutionGuard` drops after `do_solo_work` returns), and
            //    `WorkBucketStage::SecondRoots` remains closed.
            // 2. `WorkBucketStage::SecondRoots` is sequentially opened only after
            //    `CalculateForwarding` drains while mutators remain paused at GC safepoints.
            // 3. `prepare_for_roots_re_scanning` ran above before acquiring `plan_mut`,
            //    `_plan_lease` enforces non-reentrancy on this thread, `common.{release, prepare}`
            //    performs only internal space updates without calling `mmtk.get_plan()`, and
            //    `plan_mut` is dropped immediately at the end of this block.
            let plan_mut = unsafe { mmtk.get_plan_mut() }
                .downcast_mut::<Lisp2<VM>>()
                .unwrap();
            plan_mut.common.release(worker.tls, true);
            plan_mut.common.prepare(worker.tls, true);
            let _ = &solo;
        }
        #[cfg(feature = "extreme_assertions")]
        mmtk.slot_logger.reset();

        // We do two passes of transitive closures. We clear the live bytes from the first pass.
        mmtk.scheduler
            .worker_group
            .get_and_clear_worker_live_bytes();

        let tickets = VM::VMActivePlan::mutators()
            .map(MutatorTicket::from_mut)
            .collect::<Vec<_>>();
        let packets = tickets
            .into_iter()
            .map(|ticket| {
                Box::new(ScanMutatorRoots::<Lisp2ForwardingGCWorkContext<VM>>::new(
                    ticket,
                )) as _
            })
            .collect::<Vec<_>>();
        mmtk.scheduler.work_buckets[WorkBucketStage::SecondRoots].bulk_add(packets);

        mmtk.scheduler.work_buckets[WorkBucketStage::SecondRoots]
            .add(ScanVMSpecificRoots::<Lisp2ForwardingGCWorkContext<VM>>::new());
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
