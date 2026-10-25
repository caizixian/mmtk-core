#![deny(unsafe_op_in_unsafe_fn)]

//! This module contains `WorkerMonitor` and related types.  It purposes includes:
//!
//! -   letting workers become active or inactive,
//! -   letting workers park,
//! -   letting the last parked worker take action, and
//! -   letting workers and mutators notify workers when workers are given things to do.

use std::marker::PhantomData;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

use crate::util::rust_util::phase_cell::{MmtkId, WorkersParkedToken};
use crate::util::rust_util::worker_local::WorkerPerm;

use super::{
    worker::WorkerShouldExit,
    worker_goals::{WorkerGoal, WorkerGoals},
};

/// The result type of the `on_last_parked` call-back in `WorkMonitor::park_and_wait`.
/// It decides how many workers should wake up after `on_last_parked`.
pub(crate) enum LastParkedResult {
    /// The last parked worker should wait, too, until more work packets are added.
    ParkSelf,
    /// The last parked worker should unpark and find work packet to do.
    WakeSelf,
    /// Unpark ONLY the last parked worker in solo mode (`sync.solo_worker = Some(ordinal)`):
    /// all other `worker_count - 1` workers remain gated inside `park_and_wait` (including across
    /// spurious Condvar wakeups, `notify_work_available`, and `make_request`) until the solo
    /// worker drops its [`SoloExecutionGuard`].
    WakeSelfSolo,
    /// Unpark the last parked worker and up to `n - 1` others, i.e. just enough workers to cover
    /// `n` available work packets.
    Wake(usize),
    /// Wake up all parked GC workers.  For cases where which worker runs matters (designated
    /// work) or where every worker must observe a new goal (shutdown, fork).
    WakeAll,
}

/// Capability token witnessing that the current GC worker is executing a solo stage packet on
/// `instance_id` while all other `worker_count - 1` workers of `instance_id` remain parked inside
/// [`WorkerMonitor::park_and_wait`] with Condvar notifications deferred.
///
/// `!Copy`, `!Clone`, `!Send`, `!Sync`, and invariant in `'a`.
pub(crate) struct SoloWorkerToken<'a> {
    instance_id: MmtkId,
    _marker: PhantomData<*mut &'a mut ()>,
}

impl SoloWorkerToken<'_> {
    /// Return the [`MmtkId`] brand of the owning `MMTK` instance.
    #[inline(always)]
    pub(crate) const fn instance_id(&self) -> MmtkId {
        self.instance_id
    }
}

/// Unwind-safe RAII guard governing a single `WakeSelfSolo` execution window on `WorkerMonitor`.
///
/// Dropping this guard (or calling [`Self::finish`]) exits solo mode and flushes any notifications
/// that were deferred while the solo worker held exclusive access to `Plan`.
pub(crate) struct SoloExecutionGuard<'a> {
    monitor: &'a WorkerMonitor,
    ordinal: usize,
    token_issued: bool,
    armed: bool,
}

impl SoloExecutionGuard<'_> {
    /// Issue the unique [`SoloWorkerToken`] for this solo execution window, mutably borrowing
    /// `self` so solo mode cannot end until the token drops.
    #[inline]
    pub(crate) fn token(&mut self) -> SoloWorkerToken<'_> {
        assert!(
            !self.token_issued,
            "SoloExecutionGuard::token called more than once"
        );
        self.token_issued = true;
        SoloWorkerToken {
            instance_id: self.monitor.instance_id,
            _marker: PhantomData,
        }
    }

    /// Exit solo mode normally and flush deferred wakeups (forcing a broadcast if `wake_all`).
    #[inline]
    pub(crate) fn finish(mut self, wake_all: bool) {
        self.armed = false;
        self.monitor.finish_solo_execution(self.ordinal, wake_all);
    }
}

impl Drop for SoloExecutionGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            // Unwinding out of solo execution: clear `solo_worker` and broadcast so parked workers
            // are never stranded on a Condvar.
            self.monitor.finish_solo_execution(self.ordinal, true);
        }
    }
}

/// A data structure for synchronizing workers with each other and with mutators.
///
/// Unlike `GCWorkerShared`, there is only one instance of `WorkerMonitor`.
///
/// -   It allows workers to park and unpark.
/// -   It allows mutators to notify workers to schedule a GC.
pub(crate) struct WorkerMonitor {
    /// Brand of the owning `MMTK<VM>` instance.
    instance_id: MmtkId,
    /// The total number of workers.
    worker_count: usize,
    /// The synchronized part.
    sync: Mutex<WorkerMonitorSync>,
    /// Counts parked workers.  Mutated only while `sync` is held, but kept outside it so that
    /// `notify_work_available` can read it without acquiring the lock.
    parker: WorkerParker,
    /// The number of workers that are allowed to execute work after being notified.
    active_workers: AtomicUsize,
    /// Active workers wait on this when idle.  A parked *active* worker is notified if workers
    /// have things to do.  That includes:
    /// -   any work packets available, and
    /// -   any field in `sync.goals.requests` set to true.
    workers_have_anything_to_do: Condvar,
    /// Inactive workers wait on this instead.  They are notified whenever the number of active
    /// workers changes, which may turn them into active workers.
    active_worker_number_changed: Condvar,
}

/// The synchronized part of `WorkerMonitor`.
#[derive(Default)]
struct WorkerMonitorSync {
    /// Current and requested goals.
    goals: WorkerGoals,
    /// Ordinal of the sole worker currently executing a `WakeSelfSolo` stage packet, if any.
    solo_worker: Option<usize>,
    /// True once `enter_solo_packet` has claimed the current `solo_worker` window.
    solo_guard_entered: bool,
    /// Set if any `notify_work_available(true)` or `make_request` arrived during solo mode.
    deferred_notify_all: bool,
    /// Count of `notify_work_available(false)` calls deferred during solo mode.
    deferred_notify_count: usize,
}

/// This struct counts the number of workers parked and identifies the last parked worker.
struct WorkerParker {
    /// The total number of workers.
    worker_count: usize,
    /// Number of parked workers.
    /// The counter can be read without a lock, but can only be mutated while holding the monitor's lock.
    parked_workers: AtomicUsize,
}

impl WorkerParker {
    fn new(worker_count: usize) -> Self {
        Self {
            worker_count,
            parked_workers: AtomicUsize::new(0),
        }
    }

    /// Number of workers currently inside `park_and_wait`.
    /// Deliberately readable without the monitor's lock.
    fn parked_workers(&self) -> usize {
        self.parked_workers.load(Ordering::SeqCst)
    }

    /// Increase the packed-workers counter.
    /// Called before a worker is parked.  The caller must hold the monitor's lock.
    ///
    /// Return true if all the workers are parked.
    fn inc_parked_workers(&self, _sync: &WorkerMonitorSync) -> bool {
        let old = self.parked_workers.fetch_add(1, Ordering::SeqCst);
        debug_assert!(old < self.worker_count);
        old + 1 == self.worker_count
    }

    /// Decrease the packed-workers counter.
    /// Called after a worker is resumed from the parked state.  The caller must hold the
    /// monitor's lock.
    fn dec_parked_workers(&self, _sync: &WorkerMonitorSync) {
        let old = self.parked_workers.fetch_sub(1, Ordering::SeqCst);
        debug_assert!(old <= self.worker_count);
        debug_assert!(old > 0);
    }
}

/// RAII guard ensuring `WorkerParker::parked_workers` is decremented if `park_and_wait` unwinds
/// (for instance, due to a panic inside `on_last_parked`), preventing `parked_workers` from ever
/// remaining permanently inflated while `WorkerMonitor::sync` becomes poisoned.
struct ParkedWorkerGuard<'a> {
    parker: &'a WorkerParker,
    armed: bool,
}

impl<'a> ParkedWorkerGuard<'a> {
    fn new(parker: &'a WorkerParker) -> Self {
        Self {
            parker,
            armed: true,
        }
    }

    fn disarm(mut self, sync: &WorkerMonitorSync) {
        self.armed = false;
        self.parker.dec_parked_workers(sync);
    }
}

impl Drop for ParkedWorkerGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let old = self.parker.parked_workers.fetch_sub(1, Ordering::SeqCst);
            debug_assert!(old <= self.parker.worker_count);
            debug_assert!(old > 0);
        }
    }
}

impl WorkerMonitor {
    /// Construct the unique `WorkerMonitor` for `instance_id` and pre-mint its exact set of
    /// `0..worker_count` [`WorkerPerm`] capabilities.
    ///
    /// Because [`WorkerPerm::mint_for_instance`] enforces a process-global one-shot registry per
    /// [`MmtkId`], at most one `WorkerMonitor` can ever exist for `instance_id` and
    /// `self.worker_count` equals the exact total number of `WorkerPerm`s minted for `instance_id`.
    pub(crate) fn new(instance_id: MmtkId, worker_count: usize) -> (Self, Vec<WorkerPerm>) {
        assert!(
            worker_count > 0,
            "WorkerMonitor::new requires worker_count > 0"
        );
        let perms = WorkerPerm::mint_for_instance(instance_id, worker_count);
        debug_assert_eq!(perms.len(), worker_count);
        let monitor = Self {
            instance_id,
            worker_count,
            sync: Mutex::new(WorkerMonitorSync::default()),
            parker: WorkerParker::new(worker_count),
            active_workers: AtomicUsize::new(worker_count),
            workers_have_anything_to_do: Default::default(),
            active_worker_number_changed: Default::default(),
        };
        (monitor, perms)
    }

    /// Claim the active `WakeSelfSolo` execution window for `perm`'s worker and return an RAII
    /// [`SoloExecutionGuard`].
    pub(crate) fn enter_solo_packet(&self, perm: &WorkerPerm) -> SoloExecutionGuard<'_> {
        assert_eq!(
            perm.instance_id(),
            self.instance_id,
            "WorkerMonitor::enter_solo_packet MmtkId mismatch"
        );
        let ordinal = perm.ordinal();
        let mut sync = self.sync.lock().unwrap();
        assert_eq!(
            sync.solo_worker,
            Some(ordinal),
            "WorkerMonitor::enter_solo_packet called by ordinal {} while solo_worker is {:?}",
            ordinal,
            sync.solo_worker
        );
        assert!(
            !sync.solo_guard_entered,
            "WorkerMonitor::enter_solo_packet called more than once for solo window"
        );
        assert_eq!(
            self.parker.parked_workers(),
            self.worker_count - 1,
            "WorkerMonitor::enter_solo_packet expected {} parked workers",
            self.worker_count - 1
        );
        sync.solo_guard_entered = true;
        SoloExecutionGuard {
            monitor: self,
            ordinal,
            token_issued: false,
            armed: true,
        }
    }

    /// Release solo mode if a scheduled solo packet was dropped before entering `do_work`.
    pub(crate) fn cancel_unentered_solo_execution(&self, ordinal: usize) {
        self.finish_solo_execution(ordinal, true);
    }

    fn finish_solo_execution(&self, ordinal: usize, wake_all: bool) {
        let mut sync = self.sync.lock().unwrap();
        assert_eq!(
            sync.solo_worker,
            Some(ordinal),
            "WorkerMonitor::finish_solo_execution ordinal mismatch"
        );
        sync.solo_worker = None;
        sync.solo_guard_entered = false;
        let deferred_all = std::mem::replace(&mut sync.deferred_notify_all, false) || wake_all;
        let deferred_count = std::mem::replace(&mut sync.deferred_notify_count, 0);
        if deferred_all {
            self.workers_have_anything_to_do.notify_all();
            self.active_worker_number_changed.notify_all();
        } else if deferred_count > 0 {
            let max_others = self.worker_count.saturating_sub(1);
            let to_wake = deferred_count.min(max_others);
            if to_wake >= max_others {
                self.workers_have_anything_to_do.notify_all();
            } else {
                for _ in 0..to_wake {
                    self.workers_have_anything_to_do.notify_one();
                }
            }
        }
    }

    pub(crate) fn active_workers(&self) -> usize {
        self.active_workers.load(Ordering::SeqCst)
    }

    pub(crate) fn is_worker_active(&self, ordinal: usize) -> bool {
        ordinal < self.active_workers()
    }

    /// Set the number of workers that are allowed to execute work after being notified.
    ///
    /// This only updates the count.  Whenever this changes, the caller must also call
    /// `notify_work_available(true)` (directly, or indirectly via the `on_last_parked`
    /// call-back while still holding the monitor's internal lock) so that both active and
    /// inactive workers wake up and re-evaluate whether they should be active.  We don't do the
    /// notification here because this function may be called while the current thread is the
    /// last parked worker executing `on_last_parked`, in which case the notification has to
    /// happen through the already-held lock instead of acquiring it again.
    pub fn set_active_workers(&self, active_workers: usize) {
        let active_workers = active_workers.clamp(1, self.worker_count);
        self.active_workers.store(active_workers, Ordering::SeqCst);
        debug!(
            "WorkerMonitor active worker count set to {} (worker_count={}).",
            active_workers, self.worker_count
        );
    }

    /// Make a request.  Can be called by a mutator to request the workers to work towards the
    /// given `goal`.
    pub fn make_request(&self, goal: WorkerGoal) {
        // A request always needs all workers, including currently inactive ones, to make
        // progress.
        self.set_active_workers(self.worker_count);
        let mut guard = self.sync.lock().unwrap();
        let newly_requested = guard.goals.set_request(goal);
        if newly_requested {
            self.notify_work_available_while_locked(&mut guard, true);
        }
    }

    /// Wake up workers when more work packets are made available for workers,
    /// or a mutator has requested the GC workers to schedule a GC.
    pub fn notify_work_available(&self, all: bool) {
        // Fast path: no worker is parked, so there is nothing to notify and no reason to
        // serialise on the monitor lock.  Every `WorkBucket::add` during a GC comes through here,
        // so this is the difference between one load and a global lock acquisition per packet.
        if self.parker.parked_workers() == 0 {
            return;
        }
        // We must hold the lock while notifying.  Otherwise a worker that is between checking
        // its wake-up condition and actually blocking on the CondVar (both of which happen while
        // holding this lock) could have the notification delivered too early and lost, causing
        // it to block forever despite the condition it was waiting for having become true.
        let mut guard = self.sync.lock().unwrap();
        self.notify_work_available_while_locked(&mut guard, all);
    }

    /// Like `notify_work_available`, but for use by callers that already hold the monitor's
    /// internal lock (such as `park_and_wait` below), to avoid trying to lock it again.
    fn notify_work_available_while_locked(&self, sync: &mut WorkerMonitorSync, all: bool) {
        if sync.solo_worker.is_some() {
            if all {
                sync.deferred_notify_all = true;
            } else {
                sync.deferred_notify_count = sync.deferred_notify_count.saturating_add(1);
            }
            return;
        }
        if all {
            self.workers_have_anything_to_do.notify_all();
            self.active_worker_number_changed.notify_all();
        } else {
            self.workers_have_anything_to_do.notify_one();
        }
    }

    /// Wake `n` workers in total, counting the caller.  See [`LastParkedResult::Wake`].
    ///
    /// Called by the last parked worker while holding `sync`, at which point every other worker is
    /// provably blocked on `workers_have_anything_to_do`: parking requires `sync`, so each of them
    /// has already released it inside `Condvar::wait`.  `notify_one` therefore reliably wakes one
    /// real waiter per call rather than being dropped.
    fn wake_n_while_locked(&self, n: usize) {
        // The caller does not wait, so it covers one of the `n` packets itself.
        let others = n.saturating_sub(1).min(self.worker_count.saturating_sub(1));
        if others >= self.worker_count.saturating_sub(1) {
            // Waking everyone anyway; one broadcast beats N wake-ups.
            self.workers_have_anything_to_do.notify_all();
        } else {
            for _ in 0..others {
                self.workers_have_anything_to_do.notify_one();
            }
        }
    }

    /// Park a worker and wait on the CondVar `workers_have_anything_to_do`.
    ///
    /// If it is the last worker parked, `on_last_parked` will be called with a scoped
    /// [`WorkersParkedToken`] witnessing that all other workers of `self.instance_id` are parked.
    /// The return value of `on_last_parked` will determine whether this worker and other workers
    /// will wake up or block waiting.
    ///
    /// This function returns `Ok(())` if the current worker should continue working,
    /// or `Err(WorkerShouldExit)` if the current worker should exit now.
    pub fn park_and_wait<F>(
        &self,
        perm: &mut WorkerPerm,
        on_last_parked: F,
    ) -> Result<(), WorkerShouldExit>
    where
        F: for<'p> FnOnce(&mut WorkerGoals, WorkersParkedToken<'p>) -> LastParkedResult,
    {
        assert_eq!(
            perm.instance_id(),
            self.instance_id,
            "WorkerMonitor::park_and_wait MmtkId mismatch"
        );
        let ordinal = perm.ordinal();
        assert!(
            ordinal < self.worker_count,
            "WorkerMonitor::park_and_wait ordinal {} out of bounds for worker_count {}",
            ordinal,
            self.worker_count
        );

        // Never recover from a poisoned `sync` mutex (`unwrap()` panics cleanly if a prior
        // `on_last_parked` callback unwound).
        let mut sync = self.sync.lock().unwrap();
        assert!(
            sync.solo_worker != Some(ordinal),
            "WorkerMonitor::park_and_wait: solo worker {} parked before completing solo execution",
            ordinal
        );

        // Park this worker under an unwind-safe RAII guard.
        let all_parked = self.parker.inc_parked_workers(&sync);
        let park_guard = ParkedWorkerGuard::new(&self.parker);
        trace!(
            "Worker {} parked.  parked/total: {}/{}.  All parked: {}",
            ordinal,
            self.parker.parked_workers(),
            self.parker.worker_count,
            all_parked
        );

        let mut should_wait = false;

        if all_parked {
            assert!(
                sync.solo_worker.is_none(),
                "all_parked reached while solo_worker is {:?}",
                sync.solo_worker
            );
            trace!("Worker {} is the last worker parked.", ordinal);
            // SAFETY:
            // 1. `WorkerMonitor::new` calls `WorkerPerm::mint_for_instance(self.instance_id,
            //    self.worker_count)` (`worker_count >= 1`), whose global one-shot registry per
            //    `MmtkId` guarantees both that `self` is the sole `WorkerMonitor` for
            //    `self.instance_id` and that `self.worker_count` equals the exact total number of
            //    `WorkerPerm`s ever minted for `self.instance_id`.
            // 2. Every increment of `self.parker.parked_workers` occurs under `self.sync` (which
            //    is never recovered from poison) inside a `park_and_wait` call frame holding an
            //    exclusive `&mut WorkerPerm` verified to carry `self.instance_id` until after
            //    `park_guard.disarm(&sync)` (or `ParkedWorkerGuard::drop` on unwind). When
            //    `all_parked` (`old + 1 == self.worker_count`) holds, `self.worker_count` distinct
            //    `&mut WorkerPerm` borrows for `self.instance_id` are simultaneously live inside
            //    `park_and_wait`, so by the pigeonhole principle **every** `WorkerPerm` of
            //    `self.instance_id` is parked here (none can be used in `WorkerLocalSlice::get_mut`
            //    or `WorkersParkedToken::from_all_perms`), with `Release`/`Acquire` synchronization
            //    via `self.sync` from all other workers' prior memory accesses.
            // 3. `sync` remains held for the entire invariant lifetime `'p` of `parked_token`
            //    (scoped strictly to `on_last_parked`), preventing any other worker from unparking
            //    or minting another `WorkersParkedToken` during `'p`.
            let parked_token = unsafe { WorkersParkedToken::new_unchecked(self.instance_id) };
            let result = on_last_parked(&mut sync.goals, parked_token);
            match result {
                LastParkedResult::ParkSelf => {
                    should_wait = true;
                }
                LastParkedResult::WakeSelf => {
                    // Continue without waiting.
                }
                LastParkedResult::WakeSelfSolo => {
                    assert!(
                        self.is_worker_active(ordinal),
                        "Inactive worker {} cannot execute WakeSelfSolo",
                        ordinal
                    );
                    sync.solo_worker = Some(ordinal);
                    sync.solo_guard_entered = false;
                }
                LastParkedResult::Wake(n) => {
                    // We are still holding `sync`, so we must not call `notify_work_available`
                    // (which would try to lock it again and deadlock).
                    self.wake_n_while_locked(n);
                }
                LastParkedResult::WakeAll => {
                    // We are still holding `sync`, so we must not call `notify_work_available`
                    // (which would try to lock it again and deadlock).
                    self.notify_work_available_while_locked(&mut sync, true);
                }
            }
        } else {
            should_wait = true;
        }

        // Notes on CondVar usage:
        //
        // Conditional variables are usually tested in a loop while holding a mutex
        //
        //      lock();
        //      while condition() {
        //          condvar.wait();
        //      }
        //      unlock();
        //
        // The actual condition for this wait is:
        //
        // 1.  any work packet is available, or
        // 2.  a goal (such as doing GC) is requested
        //
        // But it is not used like the typical use pattern shown above, mainly because work
        // packets can be added without holding the mutex `self.sync`.  This means one worker
        // can add a new work packet (no mutex needed) right after another worker finds no work
        // packets are available and then park.  In other words, condition (1) can suddenly
        // become true after a worker sees it is false but before the worker blocks waiting on
        // the CondVar.  If this happens, the last parked worker will block forever and never
        // get notified.  This may happen if mutators or the previously existing "coordinator
        // thread" can add work packets.
        //
        // However, after the "coordinator thread" was removed, only GC worker threads can add
        // work packets during GC.  Parked workers (except the last parked worker) cannot make
        // more work packets availble (by adding new packets or opening buckets).  For this
        // reason, the **last** parked worker can be sure that after it finds no packets
        // available, no other workers can add another work packet (because they all parked).
        // So the **last** parked worker can open more buckets or declare GC finished.
        //
        // Condition (2), i.e. goals added to `sync.goals`, is guarded by the monitor `sync`.
        // When a mutator adds a goal via `WorkerMonitor::make_request`, it will notify a
        // worker; and the last parked worker always checks it before waiting.  So this
        // condition will not be set without any worker noticing.
        //
        // Note that generational barriers may add `ProcessModBuf` work packets when not in GC.
        // This is benign because those work packets are not executed immediately, and are
        // guaranteed to be executed in the next GC.
        //
        // Whether this worker is currently active is a third condition this function waits on.
        // An inactive worker waits on `active_worker_number_changed` instead of
        // `workers_have_anything_to_do`, and both are only ever notified while holding
        // `self.sync` (see `notify_work_available_while_locked`).  Because a worker holds
        // `self.sync` continuously from the point it decides to wait until the point it
        // actually blocks inside `Condvar::wait`, a notification can never be delivered to a
        // worker that has not started waiting yet: the notifier cannot acquire `self.sync`
        // (and therefore cannot notify) until the worker has released it by starting to wait.
        //
        // Notes on spurious wake-up:
        //
        // 1.  The condition variable `workers_have_anything_to_do` is guarded by `self.sync`.
        //     Because the last parked worker is holding the mutex `self.sync` when executing
        //     `on_last_parked`, no workers can unpark (even if they spuriously wake up) during
        //     `on_last_parked` because they cannot re-acquire the mutex `self.sync`.
        //
        // 2.  Workers may spuriously wake up and unpark when `on_last_parked` is not being
        //     executed (including the case when the last parked worker is waiting here, too).
        //     If one or more GC workers spuriously wake up, they will check for work packets,
        //     and park again if not available.  The last parked worker will ensure the two
        //     conditions listed above are both false before blocking.  If either condition is
        //     true, the last parked worker will take action.
        //
        // `should_wait` is `false` only when this worker is the last parked worker and
        // `on_last_parked` returned `WakeSelf` or `WakeAll`, meaning work is already known to be
        // available and this worker must proceed without waiting here.  Note that `should_wait`
        // says nothing about whether this worker is currently active: a worker that is not the
        // last parked one (`should_wait == true`) can still be inactive, e.g. if it was
        // deactivated by `set_active_workers` before it got here.  Such a worker must not wait on
        // `workers_have_anything_to_do` (it must not consume work packets while inactive), so it
        // skips straight to the `while` loop below instead.
        if should_wait && self.is_worker_active(ordinal) {
            loop {
                sync = self.workers_have_anything_to_do.wait(sync).unwrap();
                if sync.solo_worker.is_none_or(|solo| solo == ordinal) {
                    break;
                }
            }
        }

        // The worker may be inactive already (see above), may become inactive while waiting above,
        // or may need to remain parked because another worker entered `WakeSelfSolo` mode. Keep
        // waiting until this worker is active AND no other worker owns solo execution.
        while !self.is_worker_active(ordinal)
            || sync.solo_worker.is_some_and(|solo| solo != ordinal)
        {
            if !self.is_worker_active(ordinal) {
                sync = self.active_worker_number_changed.wait(sync).unwrap();
            } else {
                sync = self.workers_have_anything_to_do.wait(sync).unwrap();
            }
        }

        // Unpark this worker while still holding its exclusive `&mut WorkerPerm` borrow.
        park_guard.disarm(&sync);
        let _ = &*perm;
        trace!(
            "Worker {} unparked.  parked/total: {}/{}.",
            ordinal,
            self.parker.parked_workers(),
            self.parker.worker_count,
        );

        // If the current goal is an exit goal, the worker thread should exit.
        if matches!(
            sync.goals.current(),
            Some(WorkerGoal::Shutdown | WorkerGoal::StopForFork)
        ) {
            return Err(WorkerShouldExit);
        }

        Ok(())
    }

    /// Called when all workers have exited.
    pub fn on_all_workers_exited(&self) {
        let mut sync = self.sync.try_lock().unwrap();
        sync.goals.on_current_goal_completed();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };

    use crate::util::rust_util::phase_cell::MmtkId;

    use super::WorkerMonitor;

    /// Test if the `WorkerMonitor::park_and_wait` method calls the `on_last_parked` callback
    /// properly.
    #[test]
    fn test_last_worker_park_wake_all() {
        let number_threads = 4;
        let instance_id = MmtkId::fresh();
        let (monitor, perms) = WorkerMonitor::new(instance_id, number_threads);
        let worker_monitor = Arc::new(monitor);
        let on_last_parked_called = AtomicUsize::new(0);
        let should_unpark = AtomicBool::new(false);

        std::thread::scope(|scope| {
            for mut perm in perms {
                let ordinal = perm.ordinal();
                let worker_monitor = worker_monitor.clone();
                let on_last_parked_called = &on_last_parked_called;
                let should_unpark = &should_unpark;
                scope.spawn(move || {
                    // This emulates the use pattern in the scheduler, i.e. checking the condition
                    // ("Is there any work packets available") without holding a mutex.
                    while !should_unpark.load(Ordering::SeqCst) {
                        println!("Thread {} parking...", ordinal);
                        worker_monitor
                            .park_and_wait(&mut perm, |_goals, parked| {
                                assert_eq!(parked.instance_id(), instance_id);
                                println!("Thread {} is the last thread parked.", ordinal);
                                on_last_parked_called.fetch_add(1, Ordering::SeqCst);
                                should_unpark.store(true, Ordering::SeqCst);
                                super::LastParkedResult::WakeAll
                            })
                            .unwrap();
                        println!("Thread {} unparked.", ordinal);
                    }
                });
            }
        });

        // `on_last_parked` should only be called once.
        assert_eq!(on_last_parked_called.load(Ordering::SeqCst), 1);
    }

    /// Like `test_last_worker_park_wake_all`, but only wake up the last parked worker when it
    /// parked.
    #[test]
    fn test_last_worker_park_wake_self() {
        let number_threads = 4;
        let instance_id = MmtkId::fresh();
        let (monitor, perms) = WorkerMonitor::new(instance_id, number_threads);
        let worker_monitor = Arc::new(monitor);
        let on_last_parked_called = AtomicUsize::new(0);
        let threads_running = AtomicUsize::new(0);
        let should_unpark = AtomicBool::new(false);

        std::thread::scope(|scope| {
            for mut perm in perms {
                let ordinal = perm.ordinal();
                let worker_monitor = worker_monitor.clone();
                let on_last_parked_called = &on_last_parked_called;
                let threads_running = &threads_running;
                let should_unpark = &should_unpark;
                scope.spawn(move || {
                    let mut i_am_the_last_parked_worker = false;
                    // Record the number of threads entering the following `while` loop.
                    threads_running.fetch_add(1, Ordering::SeqCst);
                    while !should_unpark.load(Ordering::SeqCst) {
                        println!("Thread {} parking...", ordinal);
                        worker_monitor
                            .park_and_wait(&mut perm, |_goals, parked| {
                                assert_eq!(parked.instance_id(), instance_id);
                                println!("Thread {} is the last thread parked.", ordinal);
                                on_last_parked_called.fetch_add(1, Ordering::SeqCst);
                                should_unpark.store(true, Ordering::SeqCst);
                                i_am_the_last_parked_worker = true;
                                super::LastParkedResult::WakeSelf
                            })
                            .unwrap();
                        println!("Thread {} unparked.", ordinal);
                    }
                    threads_running.fetch_sub(1, Ordering::SeqCst);

                    if i_am_the_last_parked_worker {
                        println!("The last parked worker woke up");
                        // Only the current worker should wake and leave the `while` loop above.
                        assert_eq!(threads_running.load(Ordering::SeqCst), number_threads - 1);
                        should_unpark.store(true, Ordering::SeqCst);
                        worker_monitor.notify_work_available(true);
                    }
                });
            }
        });

        // `on_last_parked` should only be called once.
        assert_eq!(on_last_parked_called.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_only_selected_workers_unpark() {
        let number_threads = 4;
        let concurrent_threads = 2;
        let instance_id = MmtkId::fresh();
        let (monitor, perms) = WorkerMonitor::new(instance_id, number_threads);
        let worker_monitor = Arc::new(monitor);
        worker_monitor.set_active_workers(concurrent_threads);
        let first_wave_unparked = AtomicUsize::new(0);
        let release_everyone = AtomicBool::new(false);
        let notifier_ran = AtomicBool::new(false);

        std::thread::scope(|scope| {
            for mut perm in perms {
                let ordinal = perm.ordinal();
                let worker_monitor = worker_monitor.clone();
                let first_wave_unparked = &first_wave_unparked;
                let release_everyone = &release_everyone;
                let notifier_ran = &notifier_ran;
                scope.spawn(move || {
                    worker_monitor
                        .park_and_wait(&mut perm, |_goals, _parked| {
                            super::LastParkedResult::WakeAll
                        })
                        .unwrap();

                    if !release_everyone.load(Ordering::SeqCst) {
                        first_wave_unparked.fetch_add(1, Ordering::SeqCst);
                    }

                    if ordinal < concurrent_threads {
                        while first_wave_unparked.load(Ordering::SeqCst) < concurrent_threads {
                            std::thread::yield_now();
                        }
                        if !notifier_ran.swap(true, Ordering::SeqCst) {
                            release_everyone.store(true, Ordering::SeqCst);
                            worker_monitor.set_active_workers(number_threads);
                            worker_monitor.notify_work_available(true);
                        }
                    }
                });
            }
        });

        assert_eq!(
            first_wave_unparked.load(Ordering::SeqCst),
            concurrent_threads
        );
    }

    #[test]
    fn test_on_last_parked_panic_decrements_parked_workers_and_poisons_sync() {
        let instance_id = MmtkId::fresh();
        let (worker_monitor, mut perms) = WorkerMonitor::new(instance_id, 1);

        let panic_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = worker_monitor.park_and_wait(&mut perms[0], |_goals, _parked| {
                panic!("simulated on_last_parked panic");
            });
        }));
        assert!(panic_res.is_err());
        assert_eq!(worker_monitor.parker.parked_workers(), 0);
        assert!(worker_monitor.sync.is_poisoned());
    }

    #[test]
    fn test_solo_worker_blocks_other_workers_and_defers_notifications() {
        let number_threads = 4;
        let instance_id = MmtkId::fresh();
        let (monitor, perms) = WorkerMonitor::new(instance_id, number_threads);
        let worker_monitor = Arc::new(monitor);
        let unparked_count = AtomicUsize::new(0);
        let solo_finished = AtomicBool::new(false);

        std::thread::scope(|scope| {
            for mut perm in perms {
                let worker_monitor = worker_monitor.clone();
                let unparked_count = &unparked_count;
                let solo_finished = &solo_finished;
                scope.spawn(move || {
                    let mut is_solo = false;
                    worker_monitor
                        .park_and_wait(&mut perm, |_goals, _parked| {
                            is_solo = true;
                            super::LastParkedResult::WakeSelfSolo
                        })
                        .unwrap();

                    if is_solo {
                        let mut guard = worker_monitor.enter_solo_packet(&perm);
                        let tok = guard.token();
                        assert_eq!(tok.instance_id(), instance_id);
                        // Trigger both broadcast and direct Condvar wakeups while in solo mode:
                        // all 3 non-solo workers must remain parked until `guard.finish`.
                        worker_monitor.notify_work_available(true);
                        worker_monitor.workers_have_anything_to_do.notify_all();
                        for _ in 0..32 {
                            std::thread::yield_now();
                            assert_eq!(unparked_count.load(Ordering::SeqCst), 0);
                            assert_eq!(worker_monitor.parker.parked_workers(), number_threads - 1);
                        }
                        let _ = tok;
                        solo_finished.store(true, Ordering::SeqCst);
                        guard.finish(false);
                    } else {
                        assert!(solo_finished.load(Ordering::SeqCst));
                        unparked_count.fetch_add(1, Ordering::SeqCst);
                    }
                });
            }
        });

        assert_eq!(unparked_count.load(Ordering::SeqCst), number_threads - 1);
    }

    #[test]
    fn test_solo_worker_unwind_releases_parked_workers() {
        let number_threads = 2;
        let instance_id = MmtkId::fresh();
        let (monitor, perms) = WorkerMonitor::new(instance_id, number_threads);
        let worker_monitor = Arc::new(monitor);
        let non_solo_unparked = AtomicBool::new(false);

        std::thread::scope(|scope| {
            for mut perm in perms {
                let worker_monitor = worker_monitor.clone();
                let non_solo_unparked = &non_solo_unparked;
                scope.spawn(move || {
                    let mut is_solo = false;
                    worker_monitor
                        .park_and_wait(&mut perm, |_goals, _parked| {
                            is_solo = true;
                            super::LastParkedResult::WakeSelfSolo
                        })
                        .unwrap();

                    if is_solo {
                        let panic_res =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                let _guard = worker_monitor.enter_solo_packet(&perm);
                                panic!("simulated solo packet panic");
                            }));
                        assert!(panic_res.is_err());
                    } else {
                        non_solo_unparked.store(true, Ordering::SeqCst);
                    }
                });
            }
        });

        assert!(non_solo_unparked.load(Ordering::SeqCst));
    }
}
