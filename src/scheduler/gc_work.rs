#![deny(unsafe_op_in_unsafe_fn)]

use super::work_bucket::WorkBucketStage;
use super::*;
use crate::vm::*;
use crate::*;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::sync::atomic::Ordering;

/// Linear (`!Copy`) capability representing exclusive stop-the-world access to one paused
/// [`Mutator`].
///
/// Storing `NonNull<Mutator<VM>>` rather than `&'static mut Mutator<VM>` inside queued GC work
/// packets ends the temporary `&mut` borrow immediately when collected in `StopMutators`,
/// `Prepare`, `Release`, or `UpdateReferences`, preventing simultaneous `&mut Mutator<VM>`
/// aliasing while packets sit in work buckets or while `Plan::on_pause_start` iterates mutators.
pub struct MutatorTicket<VM: VMBinding>(NonNull<Mutator<VM>>);

// SAFETY: `MutatorTicket<VM>` transfers unique STW ownership of access to a paused `Mutator<VM>`
// (`Mutator<VM>: Send`) to a single GC worker thread for packet execution.
unsafe impl<VM: VMBinding> Send for MutatorTicket<VM> {}

impl<VM: VMBinding> MutatorTicket<VM> {
    #[inline]
    pub(crate) fn from_mut(mutator: &'static mut Mutator<VM>) -> Self {
        Self(NonNull::from(mutator))
    }

    /// Re-form an exclusive `&'static mut Mutator<VM>` during a stop-the-world work packet.
    ///
    /// # Safety
    /// The underlying mutator must remain paused at a GC safepoint for the entire lifetime of the
    /// returned reference, and no other worker, packet, or re-entrant call may concurrently access
    /// this mutator while the returned `&'static mut` borrow is live.
    #[inline]
    unsafe fn borrow_static_mut(&mut self) -> &'static mut Mutator<VM> {
        // SAFETY: Guaranteed by caller contract above; `self.0` was derived from a valid
        // `'static` `Mutator<VM>` via `NonNull::from`.
        unsafe { &mut *self.0.as_ptr() }
    }
}

/// The kind of a set of roots. Used by LXR to decide how roots should be processed
/// (e.g. reference counting and remembered-set recording).
#[repr(u8)]
#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum RootKind {
    /// Ordinary strong roots, e.g. mutator stacks and globals.  These are reference-counted and
    /// marked like any other strong reference.
    Strong,
    /// Roots held by recently JIT-compiled ("young") code-cache entries.  These are recorded into
    /// the remembered set (instead of being reference-counted) so the code cache can be
    /// re-scanned on a later GC.
    YoungCodeCacheRoots,
    /// Roots that hold weak references.  These must not keep their referents alive and are not
    /// reference-counted.
    Weak,
}

impl RootKind {
    /// Whether roots of this kind should be recorded into the remembered set rather than being
    /// processed like normal roots.
    pub fn should_record_remset(&self) -> bool {
        matches!(self, RootKind::YoungCodeCacheRoots)
    }

    /// Whether roots of this kind should skip marking and reference-count decrements.
    pub fn should_skip_mark_and_decs(&self) -> bool {
        matches!(self, RootKind::YoungCodeCacheRoots) || matches!(self, RootKind::Weak)
    }

    /// Whether roots of this kind should skip reference-count decrements.
    pub fn should_skip_decs(&self) -> bool {
        matches!(self, RootKind::YoungCodeCacheRoots) || matches!(self, RootKind::Weak)
    }
}

pub struct ScheduleCollection;

impl<VM: VMBinding> GCWork<VM> for ScheduleCollection {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, mmtk: &'static MMTK<VM>) {
        // Determine collection kind
        let is_emergency = mmtk.state.set_collection_kind(
            mmtk.get_plan().last_collection_was_exhaustive(),
            mmtk.gc_trigger.policy.can_heap_size_grow(),
        );
        if is_emergency {
            mmtk.get_plan().notify_emergency_collection();
        }
        mmtk.state.stacks_prepared.store(false, Ordering::SeqCst);
        mmtk.state
            .mutators_prepared_in_scan
            .store(false, Ordering::Release);
        // FIXME: This seems to be a weird place to start counting GC statistics.
        // This is not when we set the status to PauseRequested or InPause. Instead, this is just a random place during transition
        // See https://github.com/mmtk/mmtk-core/issues/1330
        mmtk.stats.start_gc();

        // Let the plan to schedule collection work
        mmtk.get_plan().schedule_collection(worker.scheduler());
    }
}

/// The global GC Preparation Work.
///
/// Constructing `Prepare::<C>::new(plan)` registers [`PrepareSolo`] as solo stage work for
/// [`WorkBucketStage::Prepare`] so `Plan::prepare` executes exclusively on a single worker before
/// `WorkBucketStage::Prepare` opens for parallel root/mutator packets. The `Prepare` struct added
/// to `work_buckets[WorkBucketStage::Prepare]` serves as a stage marker.
pub struct Prepare<C: GCWorkContext> {
    _p: PhantomData<fn() -> C>,
}

impl<C: GCWorkContext> Prepare<C> {
    pub fn new(plan: &C::PlanType) -> Self {
        plan.base().scheduler.set_solo_stage_work(
            WorkBucketStage::Prepare,
            PrepareSolo::<C> {
                plan_addr: plan as *const C::PlanType as usize,
                _p: PhantomData,
            },
        );
        Self { _p: PhantomData }
    }
}

impl<C: GCWorkContext> GCWork<C::VM> for Prepare<C> {
    fn do_work(&mut self, _worker: &mut GCWorker<C::VM>, _mmtk: &'static MMTK<C::VM>) {}
}

struct PrepareSolo<C: GCWorkContext> {
    plan_addr: usize,
    _p: PhantomData<fn() -> C>,
}

impl<C: GCWorkContext> SoloStageWork<C::VM> for PrepareSolo<C> {
    fn do_solo_work(
        &mut self,
        worker: &mut GCWorker<C::VM>,
        mmtk: &'static MMTK<C::VM>,
        solo: SoloWorkerToken<'_>,
    ) {
        trace!("Prepare Global");
        assert_eq!(solo.instance_id(), mmtk.id);
        {
            let _plan_lease = mmtk.enter_plan_mut_lease();
            // SAFETY:
            // 1. `solo: SoloWorkerToken<'_>` (`solo.instance_id() == mmtk.id`) is minted solely by
            //    `WorkerMonitor::enter_solo_packet` after `all_parked` locked all other
            //    `worker_count - 1` `WorkerPerm`s of `mmtk.id` inside `WorkerMonitor::park_and_wait`
            //    (gated across spurious Condvar wakeups, with `notify_work_available` and
            //    `make_request` deferred until `SoloExecutionGuard` drops after `do_solo_work`
            //    returns), and `WorkBucketStage::Prepare` remains closed so no pre-populated
            //    packets have started.
            // 2. `WorkBucketStage::Prepare` is sequentially opened only after `FIRST_STW_STAGE` was
            //    opened by `notify_mutators_paused`, so every mutator is paused at a GC safepoint.
            // 3. `_plan_lease` enforces non-reentrancy on this thread, `Plan::prepare` performs
            //    only internal plan/space preparation without calling `mmtk.get_plan()` or VM
            //    callbacks, and `plan_mut` is dropped immediately at the end of this block before
            //    `mmtk.get_plan().constraints()` is read below (preserving Stacked/Tree Borrows for
            //    any `'static` space references queued into packets during `prepare`).
            let plan_mut: &mut C::PlanType = unsafe { mmtk.get_plan_mut() }
                .downcast_mut::<C::PlanType>()
                .unwrap();
            debug_assert_eq!(self.plan_addr, plan_mut as *const C::PlanType as usize);
            plan_mut.prepare(worker.tls);
            let _ = &solo;
        }

        let needs_prepare_mutator = mmtk.get_plan().constraints().needs_prepare_mutator;
        if needs_prepare_mutator && !mmtk.state.mutators_prepared_in_scan.load(Ordering::Acquire) {
            let tickets = <C::VM as VMBinding>::VMActivePlan::mutators()
                .map(MutatorTicket::from_mut)
                .collect::<Vec<_>>();
            // Just in case the VM binding is inconsistent about the number of mutators and the actual mutator list.
            debug_assert_eq!(
                tickets.len(),
                <C::VM as VMBinding>::VMActivePlan::number_of_mutators()
            );
            let prepare_mutator_packets = tickets
                .into_iter()
                .map(|ticket| Box::new(PrepareMutator::<C::VM>::new(ticket)) as _)
                .collect::<Vec<_>>();
            mmtk.scheduler.work_buckets[WorkBucketStage::Prepare].bulk_add(prepare_mutator_packets);
        }

        for w in &mmtk.scheduler.worker_group.workers_shared {
            let result = w.designated_work.push(Box::new(PrepareCollector));
            debug_assert!(result.is_ok());
        }
    }
}

/// The mutator GC Preparation Work
pub struct PrepareMutator<VM: VMBinding> {
    ticket: Option<MutatorTicket<VM>>,
}

impl<VM: VMBinding> PrepareMutator<VM> {
    pub(crate) fn new(ticket: MutatorTicket<VM>) -> Self {
        Self {
            ticket: Some(ticket),
        }
    }
}

impl<VM: VMBinding> GCWork<VM> for PrepareMutator<VM> {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, _mmtk: &'static MMTK<VM>) {
        trace!("Prepare Mutator");
        let mut ticket = self
            .ticket
            .take()
            .expect("PrepareMutator executed more than once");
        // SAFETY: Mutators remain paused throughout `WorkBucketStage::Prepare`, `ticket` was
        // minted uniquely for this mutator, and `PrepareMutator` is only scheduled when
        // `ScanMutatorRoots` is not concurrently in `WorkBucketStage::Prepare` (otherwise fused
        // into `PrepareAndScanMutator`).
        unsafe { ticket.borrow_static_mut() }.prepare(worker.tls);
    }
}

/// The collector GC Preparation Work
#[derive(Default)]
pub struct PrepareCollector;

impl<VM: VMBinding> GCWork<VM> for PrepareCollector {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, mmtk: &'static MMTK<VM>) {
        trace!("Prepare Collector");
        worker.get_copy_context_mut().prepare();
        mmtk.get_plan().prepare_worker(worker);
    }
}

/// The global GC release Work.
///
/// Constructing `Release::<C>::new(plan)` registers [`ReleaseSolo`] as solo stage work for
/// [`WorkBucketStage::Release`] so `Plan::release` executes exclusively on a single worker before
/// `WorkBucketStage::Release` opens for parallel `ReleaseMutator` / weak-ref / unlog-bit packets.
pub struct Release<C: GCWorkContext> {
    _p: PhantomData<fn() -> C>,
}

impl<C: GCWorkContext> Release<C> {
    pub fn new(plan: &C::PlanType) -> Self {
        plan.base().scheduler.set_solo_stage_work(
            WorkBucketStage::Release,
            ReleaseSolo::<C> {
                plan_addr: plan as *const C::PlanType as usize,
                _p: PhantomData,
            },
        );
        Self { _p: PhantomData }
    }
}

impl<C: GCWorkContext + 'static> GCWork<C::VM> for Release<C> {
    fn do_work(&mut self, _worker: &mut GCWorker<C::VM>, _mmtk: &'static MMTK<C::VM>) {}
}

struct ReleaseSolo<C: GCWorkContext> {
    plan_addr: usize,
    _p: PhantomData<fn() -> C>,
}

impl<C: GCWorkContext> SoloStageWork<C::VM> for ReleaseSolo<C> {
    fn do_solo_work(
        &mut self,
        worker: &mut GCWorker<C::VM>,
        mmtk: &'static MMTK<C::VM>,
        solo: SoloWorkerToken<'_>,
    ) {
        trace!("Release Global");
        assert_eq!(solo.instance_id(), mmtk.id);

        mmtk.gc_trigger.policy.on_gc_release(mmtk);
        crate::plan::release_before_plan_mut(mmtk.get_plan());
        {
            let _plan_lease = mmtk.enter_plan_mut_lease();
            // SAFETY:
            // 1. `solo: SoloWorkerToken<'_>` (`solo.instance_id() == mmtk.id`) is minted solely by
            //    `WorkerMonitor::enter_solo_packet` after `all_parked` locked all other
            //    `worker_count - 1` `WorkerPerm`s of `mmtk.id` inside `WorkerMonitor::park_and_wait`
            //    (gated across spurious Condvar wakeups, with `notify_work_available` and
            //    `make_request` deferred until `SoloExecutionGuard` drops after `do_solo_work`
            //    returns), and `WorkBucketStage::Release` remains closed so no pre-populated
            //    packets (`RefEnqueue`, `VMPostForwarding`, `ClearCommonPlanUnlogBits`) have
            //    started.
            // 2. `WorkBucketStage::Release` is sequentially opened after all prior STW stages have
            //    drained while mutators remain paused at GC safepoints.
            // 3. `on_gc_release` and `release_before_plan_mut` (`LXR` VM callbacks) ran above
            //    before acquiring `plan_mut`, `_plan_lease` enforces non-reentrancy on this thread,
            //    `release_plan_local` performs only internal plan/space release updates, and
            //    `plan_mut` is dropped immediately at the end of this block before
            //    `release_after_plan_mut` runs (preserving Stacked/Tree Borrows for any `'static`
            //    space references queued into packets during `release`).
            let plan_mut: &mut C::PlanType = unsafe { mmtk.get_plan_mut() }
                .downcast_mut::<C::PlanType>()
                .unwrap();
            debug_assert_eq!(self.plan_addr, plan_mut as *const C::PlanType as usize);
            crate::plan::release_plan_local::<C::PlanType>(plan_mut, worker.tls);
            let _ = &solo;
        }
        crate::plan::release_after_plan_mut(mmtk.get_plan());

        let tickets = <C::VM as VMBinding>::VMActivePlan::mutators()
            .map(MutatorTicket::from_mut)
            .collect::<Vec<_>>();
        // Just in case the VM binding is inconsistent about the number of mutators and the actual mutator list.
        debug_assert_eq!(
            tickets.len(),
            <C::VM as VMBinding>::VMActivePlan::number_of_mutators()
        );
        let release_mutator_packets = tickets
            .into_iter()
            .map(|ticket| Box::new(ReleaseMutator::<C::VM>::new(ticket)) as _)
            .collect::<Vec<_>>();
        mmtk.scheduler.work_buckets[WorkBucketStage::Release].bulk_add(release_mutator_packets);

        for w in &mmtk.scheduler.worker_group.workers_shared {
            let result = w.designated_work.push(Box::new(ReleaseCollector));
            debug_assert!(result.is_ok());
        }
    }
}

/// The mutator release Work
pub struct ReleaseMutator<VM: VMBinding> {
    ticket: Option<MutatorTicket<VM>>,
}

impl<VM: VMBinding> ReleaseMutator<VM> {
    pub(crate) fn new(ticket: MutatorTicket<VM>) -> Self {
        Self {
            ticket: Some(ticket),
        }
    }
}

impl<VM: VMBinding> GCWork<VM> for ReleaseMutator<VM> {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, _mmtk: &'static MMTK<VM>) {
        trace!("Release Mutator");
        let mut ticket = self
            .ticket
            .take()
            .expect("ReleaseMutator executed more than once");
        // SAFETY: `WorkBucketStage::Release` runs while mutators remain paused after all prior STW
        // stages have drained, and each mutator's `ReleaseMutator` packet owns a unique `ticket`.
        unsafe { ticket.borrow_static_mut() }.release(worker.tls);
    }
}

/// The collector release Work
#[derive(Default)]
pub struct ReleaseCollector;

impl<VM: VMBinding> GCWork<VM> for ReleaseCollector {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, _mmtk: &'static MMTK<VM>) {
        trace!("Release Collector");
        worker.get_copy_context_mut().release();
    }
}

/// Stop all mutators
///
/// TODO: Smaller work granularity
#[derive(Default)]
pub struct StopMutators<C: GCWorkContext> {
    /// If this is true, we skip creating [`ScanMutatorRoots`] work packets for mutators.
    /// By default, this is false.
    skip_mutator_roots: bool,
    /// If this is true, we skip scanning VM-specific roots.
    /// By default, this is false.
    skip_vm_roots: bool,
    /// Flush mutators once they are stopped. By default this is false. [`ScanMutatorRoots`] will flush mutators.
    flush_mutator: bool,
    phantom: PhantomData<C>,
}

impl<C: GCWorkContext> StopMutators<C> {
    pub fn new() -> Self {
        Self {
            skip_mutator_roots: false,
            skip_vm_roots: false,
            flush_mutator: false,
            phantom: PhantomData,
        }
    }

    pub fn new_with_flush() -> Self {
        let mut me = Self::new();
        me.flush_mutator = true;
        me
    }

    /// Create a `StopMutators` work packet that does not create any root-scanning work packets, and will simply flush mutators.
    pub fn new_no_scan_roots() -> Self {
        Self {
            skip_mutator_roots: true,
            skip_vm_roots: true,
            flush_mutator: true,
            phantom: PhantomData,
        }
    }
}

impl<C: GCWorkContext> GCWork<C::VM> for StopMutators<C> {
    fn do_work(&mut self, worker: &mut GCWorker<C::VM>, mmtk: &'static MMTK<C::VM>) {
        trace!("stop_all_mutators start");
        mmtk.state.prepare_for_stack_scanning();
        let mut tickets: Vec<MutatorTicket<C::VM>> = Vec::new();
        <C::VM as VMBinding>::VMCollection::stop_all_mutators(worker.tls, |mutator| {
            if self.flush_mutator {
                mutator.flush();
            }
            if !self.skip_mutator_roots {
                // Convert `mutator` to a raw `NonNull` ticket immediately so the `&mut Mutator`
                // borrow ends before `stop_all_mutators` visits the next mutator or `on_pause_start`
                // runs below.
                tickets.push(MutatorTicket::from_mut(mutator));
            }
        });
        trace!("stop_all_mutators end");
        // All mutators have just stopped: this is the end of the "time-to-yield" window
        mmtk.stats
            .record_time_to_yield(mmtk.state.take_time_to_yield());
        mmtk.state.record_pause_start_time();

        let reserved_pages = mmtk.get_plan().get_reserved_pages();
        let total_pages = mmtk.get_plan().get_total_pages();
        probe!(mmtk, heap_stats, reserved_pages, total_pages);

        // Run `on_pause_start` (e.g. `LXR::on_pause_start`) before opening STW buckets or
        // materializing any per-mutator packet's `&mut Mutator`.
        mmtk.get_plan().on_pause_start(mmtk);

        let root_scanning_stage = mmtk.get_plan().root_scanning_stage();
        let fuse_prepare_and_scan = !self.skip_mutator_roots
            && !tickets.is_empty()
            && root_scanning_stage == WorkBucketStage::Prepare
            && mmtk.get_plan().constraints().needs_prepare_mutator;
        mmtk.state
            .mutators_prepared_in_scan
            .store(fuse_prepare_and_scan, Ordering::Release);

        if fuse_prepare_and_scan {
            let packets = tickets
                .into_iter()
                .map(|ticket| Box::new(PrepareAndScanMutator::<C>::new(ticket)) as _)
                .collect::<Vec<_>>();
            mmtk.scheduler.work_buckets[WorkBucketStage::Prepare].bulk_add(packets);
        } else if !tickets.is_empty() {
            let packets = tickets
                .into_iter()
                .map(|ticket| Box::new(ScanMutatorRoots::<C>::new(ticket)) as _)
                .collect::<Vec<_>>();
            mmtk.scheduler.work_buckets[root_scanning_stage].bulk_add(packets);
        }

        mmtk.scheduler.notify_mutators_paused(mmtk);
        // Tell GC trigger that the pause started.
        mmtk.gc_trigger.policy.on_pause_start(mmtk);
        if !self.skip_vm_roots {
            let factory = C::make_roots_work_factory(mmtk);
            <C::VM as VMBinding>::VMScanning::scan_vm_specific_roots(worker.tls, factory);
        }
    }
}

/// Scan roots (optionally preparing first) and flush buffers for a single paused mutator using a
/// linear [`MutatorTicket`].
///
/// # Safety
/// `ticket` must uniquely own STW access to its paused `Mutator<C::VM>` for the duration of this
/// call, with no concurrent packet accessing the same mutator in the open bucket stage.
unsafe fn scan_mutator_roots_with_ticket<C: GCWorkContext>(
    mut ticket: MutatorTicket<C::VM>,
    worker: &mut GCWorker<C::VM>,
    mmtk: &'static MMTK<C::VM>,
    prepare_first: bool,
) {
    // SAFETY: Caller guarantees `ticket` uniquely owns STW access to this paused `'static`
    // `Mutator<C::VM>`, and `Scanning::scan_roots_in_mutator_thread` consumes `mutator`
    // synchronously without retaining the `&'static mut` reference past its return.
    let mutator = unsafe { ticket.borrow_static_mut() };
    if prepare_first {
        mutator.prepare(worker.tls);
    }
    let mutator_tls = mutator.get_tls();
    trace!("ScanMutatorRoots for mutator {:?}", mutator_tls);
    let mutators = <C::VM as VMBinding>::VMActivePlan::number_of_mutators();
    let factory = C::make_roots_work_factory(mmtk);
    <C::VM as VMBinding>::VMScanning::scan_roots_in_mutator_thread(worker.tls, mutator, factory);
    // SAFETY: `scan_roots_in_mutator_thread` has returned without retaining the mutator borrow,
    // and `ticket` remains the sole owner of STW access to this mutator for the rest of this call.
    unsafe { ticket.borrow_static_mut() }.flush();

    if mmtk.state.inform_stack_scanned(mutators) {
        <C::VM as VMBinding>::VMScanning::notify_initial_thread_scan_complete(false, worker.tls);
    }
}

/// Fused per-mutator `PrepareMutator` + `ScanMutatorRoots` packet used when both execute in
/// `WorkBucketStage::Prepare`, preventing two parallel workers in `Prepare` from concurrently
/// borrowing the same `Mutator<VM>`.
pub(crate) struct PrepareAndScanMutator<C: GCWorkContext> {
    ticket: Option<MutatorTicket<C::VM>>,
}

impl<C: GCWorkContext> PrepareAndScanMutator<C> {
    pub(crate) fn new(ticket: MutatorTicket<C::VM>) -> Self {
        Self {
            ticket: Some(ticket),
        }
    }
}

impl<C: GCWorkContext> GCWork<C::VM> for PrepareAndScanMutator<C> {
    fn do_work(&mut self, worker: &mut GCWorker<C::VM>, mmtk: &'static MMTK<C::VM>) {
        let ticket = self
            .ticket
            .take()
            .expect("PrepareAndScanMutator executed more than once");
        trace!("PrepareAndScanMutator");
        // SAFETY: Mutators are paused, `Prepare::do_work`'s separate `PrepareMutator` loop is
        // suppressed via `mutators_prepared_in_scan`, and `ticket` uniquely owns this mutator
        // throughout this packet.
        unsafe { scan_mutator_roots_with_ticket::<C>(ticket, worker, mmtk, true) };
    }
}

pub struct ScanMutatorRoots<C: GCWorkContext>(Option<MutatorTicket<C::VM>>);

impl<C: GCWorkContext> ScanMutatorRoots<C> {
    pub(crate) fn new(ticket: MutatorTicket<C::VM>) -> Self {
        Self(Some(ticket))
    }
}

impl<C: GCWorkContext> GCWork<C::VM> for ScanMutatorRoots<C> {
    fn do_work(&mut self, worker: &mut GCWorker<C::VM>, mmtk: &'static MMTK<C::VM>) {
        let ticket = self
            .0
            .take()
            .expect("ScanMutatorRoots executed more than once");
        // SAFETY: Mutators are paused, `ticket` uniquely owns this mutator in the current bucket
        // stage (`ScanMutatorRoots` is only scheduled when `PrepareMutator` is either disabled or
        // placed in a barrier-disjoint stage such as `Prepare` vs `RCProcessIncs` / `SecondRoots`),
        // and `Option::take` ensures single execution.
        unsafe { scan_mutator_roots_with_ticket::<C>(ticket, worker, mmtk, false) };
    }
}

#[derive(Default)]
pub struct ScanVMSpecificRoots<C: GCWorkContext>(PhantomData<C>);

impl<C: GCWorkContext> ScanVMSpecificRoots<C> {
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

impl<C: GCWorkContext> GCWork<C::VM> for ScanVMSpecificRoots<C> {
    fn do_work(&mut self, worker: &mut GCWorker<C::VM>, mmtk: &'static MMTK<C::VM>) {
        trace!("ScanStaticRoots");
        let factory = C::make_roots_work_factory(mmtk);
        <C::VM as VMBinding>::VMScanning::scan_vm_specific_roots(worker.tls, factory);
    }
}
