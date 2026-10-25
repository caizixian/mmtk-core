//! This module contains `WorkerMonitor` and related types.  It purposes includes:
//!
//! -   letting workers become active or inactive,
//! -   letting workers park,
//! -   letting the last parked worker take action, and
//! -   letting workers and mutators notify workers when workers are given things to do.

use std::cell::Cell;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

use super::{
    worker::WorkerShouldExit,
    worker_goals::{WorkerGoal, WorkerGoals},
};

thread_local! {
    /// Non-null while the current thread holds `WorkerMonitor::sync` inside `on_last_parked`.
    /// Suppresses re-entrant `sync.lock()` in `notify_work_available` when a pre-open hook adds
    /// packets to an open bucket (`park_and_wait` performs the wakeup as `on_last_parked` returns).
    static IN_LAST_PARKED: Cell<*const WorkerMonitor> = const { Cell::new(std::ptr::null()) };
}

struct LastParkedScope {
    prev: *const WorkerMonitor,
}

impl LastParkedScope {
    fn enter(monitor: &WorkerMonitor) -> Self {
        let prev = IN_LAST_PARKED.with(|cur| cur.replace(monitor));
        Self { prev }
    }
}

impl Drop for LastParkedScope {
    fn drop(&mut self) {
        IN_LAST_PARKED.with(|cur| cur.set(self.prev));
    }
}

/// Scoped proof that all GC workers of the referenced [`WorkerMonitor`] are currently parked inside
/// [`WorkerMonitor::park_and_wait`] with `WorkerMonitor::sync` held by the calling thread.
///
/// `!Send`, `!Sync`, and non-`Clone` so it cannot escape the `on_last_parked` callback frame.
pub(crate) struct WorkersParked<'a> {
    monitor: &'a WorkerMonitor,
    _not_send_sync: PhantomData<*mut ()>,
}

impl WorkersParked<'_> {
    #[inline]
    pub(crate) fn monitor_eq(&self, other: &WorkerMonitor) -> bool {
        std::ptr::eq(self.monitor, other)
    }
}

/// The result type of the `on_last_parked` call-back in `WorkMonitor::park_and_wait`.
/// It decides how many workers should wake up after `on_last_parked`.
pub(crate) enum LastParkedResult {
    /// The last parked worker should wait, too, until more work packets are added.
    ParkSelf,
    /// The last parked worker should unpark and find work packet to do.
    WakeSelf,
    /// Unpark the last parked worker and up to `n - 1` others, i.e. just enough workers to cover
    /// `n` available work packets.
    Wake(usize),
    /// Wake up all parked GC workers.  For cases where which worker runs matters (designated
    /// work) or where every worker must observe a new goal (shutdown, fork).
    WakeAll,
}

/// A data structure for synchronizing workers with each other and with mutators.
///
/// Unlike `GCWorkerShared`, there is only one instance of `WorkerMonitor`.
///
/// -   It allows workers to park and unpark.
/// -   It allows mutators to notify workers to schedule a GC.
pub(crate) struct WorkerMonitor {
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
struct WorkerMonitorSync {
    /// Current and requested goals.
    goals: WorkerGoals,
    /// Tracks which distinct worker ordinals (`0..worker_count`) are currently parked inside
    /// `park_and_wait`.
    parked_by_ordinal: Box<[bool]>,
}

impl WorkerMonitorSync {
    fn new(worker_count: usize) -> Self {
        Self {
            goals: WorkerGoals::default(),
            parked_by_ordinal: vec![false; worker_count].into_boxed_slice(),
        }
    }
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
    ordinal: usize,
    armed: bool,
}

impl<'a> ParkedWorkerGuard<'a> {
    fn new(parker: &'a WorkerParker, ordinal: usize) -> Self {
        Self {
            parker,
            ordinal,
            armed: true,
        }
    }

    fn disarm(mut self, sync: &mut WorkerMonitorSync) {
        self.armed = false;
        debug_assert!(sync.parked_by_ordinal[self.ordinal]);
        sync.parked_by_ordinal[self.ordinal] = false;
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
    pub(crate) fn new(worker_count: usize) -> Self {
        assert!(
            worker_count > 0,
            "WorkerMonitor::new requires worker_count > 0"
        );
        Self {
            worker_count,
            sync: Mutex::new(WorkerMonitorSync::new(worker_count)),
            parker: WorkerParker::new(worker_count),
            active_workers: AtomicUsize::new(worker_count),
            workers_have_anything_to_do: Default::default(),
            active_worker_number_changed: Default::default(),
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
        if IN_LAST_PARKED.with(|cur| std::ptr::eq(cur.get(), self)) {
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
    fn notify_work_available_while_locked(&self, _sync: &mut WorkerMonitorSync, all: bool) {
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
    /// Holding `&mut GCWorker<VM>` across `park_and_wait` statically guarantees this worker is not
    /// concurrently executing any `GCWork` packet; if it is the last worker parked,
    /// `on_last_parked` will be called with a scoped [`WorkersParked`] witness while `self.sync`
    /// remains locked.
    pub(super) fn park_and_wait<VM: crate::vm::VMBinding, F>(
        &self,
        worker: &mut super::worker::GCWorker<VM>,
        on_last_parked: F,
    ) -> Result<(), WorkerShouldExit>
    where
        F: for<'p> FnOnce(&mut WorkerGoals, &mut WorkersParked<'p>) -> LastParkedResult,
    {
        assert!(
            std::ptr::eq(self, worker.scheduler().worker_monitor.as_ref()),
            "GCWorker belongs to a different WorkerMonitor"
        );
        self.park_and_wait_by_ordinal(worker.ordinal, on_last_parked)
    }

    fn park_and_wait_by_ordinal<F>(
        &self,
        ordinal: usize,
        on_last_parked: F,
    ) -> Result<(), WorkerShouldExit>
    where
        F: for<'p> FnOnce(&mut WorkerGoals, &mut WorkersParked<'p>) -> LastParkedResult,
    {
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
            !sync.parked_by_ordinal[ordinal],
            "WorkerMonitor::park_and_wait duplicate park for ordinal {}",
            ordinal
        );
        sync.parked_by_ordinal[ordinal] = true;

        // Park this worker under an unwind-safe RAII guard.
        let all_parked = self.parker.inc_parked_workers(&sync);
        let park_guard = ParkedWorkerGuard::new(&self.parker, ordinal);
        trace!(
            "Worker {} parked.  parked/total: {}/{}.  All parked: {}",
            ordinal,
            self.parker.parked_workers(),
            self.parker.worker_count,
            all_parked
        );

        let mut should_wait = false;

        if all_parked {
            debug_assert!(sync.parked_by_ordinal.iter().all(|&p| p));
            trace!("Worker {} is the last worker parked.", ordinal);
            let result = {
                let _scope = LastParkedScope::enter(self);
                let mut parked = WorkersParked {
                    monitor: self,
                    _not_send_sync: PhantomData,
                };
                on_last_parked(&mut sync.goals, &mut parked)
            };
            match result {
                LastParkedResult::ParkSelf => {
                    should_wait = true;
                }
                LastParkedResult::WakeSelf => {
                    // Continue without waiting.
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
            sync = self.workers_have_anything_to_do.wait(sync).unwrap();
        }

        // The worker may be inactive already (see above) or may become inactive while waiting
        // above. Keep waiting until this worker is active.
        while !self.is_worker_active(ordinal) {
            sync = self.active_worker_number_changed.wait(sync).unwrap();
        }

        park_guard.disarm(&mut sync);
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

    use super::WorkerMonitor;

    /// Test if the `WorkerMonitor::park_and_wait` method calls the `on_last_parked` callback
    /// properly.
    #[test]
    fn test_last_worker_park_wake_all() {
        let number_threads = 4;
        let worker_monitor = Arc::new(WorkerMonitor::new(number_threads));
        let on_last_parked_called = AtomicUsize::new(0);
        let should_unpark = AtomicBool::new(false);

        std::thread::scope(|scope| {
            for ordinal in 0..number_threads {
                let worker_monitor = worker_monitor.clone();
                let on_last_parked_called = &on_last_parked_called;
                let should_unpark = &should_unpark;
                scope.spawn(move || {
                    while !should_unpark.load(Ordering::SeqCst) {
                        println!("Thread {} parking...", ordinal);
                        worker_monitor
                            .park_and_wait_by_ordinal(ordinal, |_goals, parked| {
                                assert!(parked.monitor_eq(&worker_monitor));
                                // Verify re-entrant `notify_work_available` inside `on_last_parked`
                                // does not deadlock on `sync`.
                                worker_monitor.notify_work_available(true);
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

        assert_eq!(on_last_parked_called.load(Ordering::SeqCst), 1);
    }

    /// Like `test_last_worker_park_wake_all`, but only wake up the last parked worker when it
    /// parked.
    #[test]
    fn test_last_worker_park_wake_self() {
        let number_threads = 4;
        let worker_monitor = Arc::new(WorkerMonitor::new(number_threads));
        let on_last_parked_called = AtomicUsize::new(0);
        let threads_running = AtomicUsize::new(0);
        let should_unpark = AtomicBool::new(false);

        std::thread::scope(|scope| {
            for ordinal in 0..number_threads {
                let worker_monitor = worker_monitor.clone();
                let on_last_parked_called = &on_last_parked_called;
                let threads_running = &threads_running;
                let should_unpark = &should_unpark;
                scope.spawn(move || {
                    let mut i_am_the_last_parked_worker = false;
                    threads_running.fetch_add(1, Ordering::SeqCst);
                    while !should_unpark.load(Ordering::SeqCst) {
                        println!("Thread {} parking...", ordinal);
                        worker_monitor
                            .park_and_wait_by_ordinal(ordinal, |_goals, parked| {
                                assert!(parked.monitor_eq(&worker_monitor));
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
                        assert_eq!(threads_running.load(Ordering::SeqCst), number_threads - 1);
                        should_unpark.store(true, Ordering::SeqCst);
                        worker_monitor.notify_work_available(true);
                    }
                });
            }
        });

        assert_eq!(on_last_parked_called.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_only_selected_workers_unpark() {
        let number_threads = 4;
        let concurrent_threads = 2;
        let worker_monitor = Arc::new(WorkerMonitor::new(number_threads));
        worker_monitor.set_active_workers(concurrent_threads);
        let first_wave_unparked = AtomicUsize::new(0);
        let release_everyone = AtomicBool::new(false);
        let notifier_ran = AtomicBool::new(false);

        std::thread::scope(|scope| {
            for ordinal in 0..number_threads {
                let worker_monitor = worker_monitor.clone();
                let first_wave_unparked = &first_wave_unparked;
                let release_everyone = &release_everyone;
                let notifier_ran = &notifier_ran;
                scope.spawn(move || {
                    worker_monitor
                        .park_and_wait_by_ordinal(ordinal, |_goals, _parked| {
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
        let worker_monitor = WorkerMonitor::new(1);

        let panic_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = worker_monitor.park_and_wait_by_ordinal(0, |_goals, _parked| {
                panic!("simulated on_last_parked panic");
            });
        }));
        assert!(panic_res.is_err());
        assert_eq!(worker_monitor.parker.parked_workers(), 0);
        assert!(worker_monitor.sync.is_poisoned());
    }
}
