use super::MutatorHandleRef;
use crate::util::VMThread;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex};

#[derive(Clone)]
pub(super) struct ThreadPark {
    name: &'static str,
    inner: Arc<Inner>,
}

struct Inner {
    lock: Mutex<State>,
    cvar: Condvar,
}

#[derive(Default)]
struct State {
    /// All registered mutator threads mapped directly to their `'static` `MutatorHandleRef` (no
    /// raw pointer casting from `VMThread` needed).
    parked: HashMap<VMThread, MutatorHandleRef>,
}

impl ThreadPark {
    pub(super) fn new(name: &'static str) -> Self {
        Self {
            name,
            inner: Arc::new(Inner {
                lock: Mutex::new(State::default()),
                cvar: Condvar::new(),
            }),
        }
    }

    /// Register `tid` with its `'static` `MutatorHandleRef`.
    pub(super) fn register(&self, tid: VMThread, handle: MutatorHandleRef) {
        debug!("Register {:?} to {}", tid, self.name);
        let mut state = self.inner.lock.lock().unwrap();
        let prev = state.parked.insert(tid, handle);
        assert!(
            prev.is_none(),
            "Duplicate registration of {:?} in {}",
            tid,
            self.name
        );
    }

    pub(super) fn contains(&self, tid: VMThread) -> bool {
        let state = self.inner.lock.lock().unwrap();
        state.parked.contains_key(&tid)
    }

    /// Cold-path ownership transfer when a `GenericMutatorFixture` moves across OS threads:
    /// atomically verifies `tid` is registered and NOT parked under the `ThreadPark` lock (so it
    /// cannot race with `park`), then updates `handle.0.active_thread` to `new_owner`.
    pub(super) fn transfer_owner(&self, tid: VMThread, new_owner: usize) {
        let state = self.inner.lock.lock().unwrap();
        let handle = state.parked.get(&tid).copied().unwrap_or_else(|| {
            panic!("Thread {:?} is not registered in {}", tid, self.name);
        });
        assert!(
            !handle.0.is_parked.load(Ordering::Acquire),
            "Cannot borrow fixture mutator {:?} while it is parked in {}",
            tid,
            self.name
        );
        handle.0.active_thread.store(new_owner, Ordering::Release);
    }

    pub(super) fn number_of_threads(&self) -> usize {
        let state = self.inner.lock.lock().unwrap();
        state.parked.len()
    }

    /// Return all registered `'static` `MutatorHandleRef`s, asserting that every registered
    /// mutator thread is currently parked at its safepoint.
    pub(super) fn all_parked_handles(&self) -> Vec<MutatorHandleRef> {
        let state = self.inner.lock.lock().unwrap();
        assert!(
            state
                .parked
                .values()
                .all(|handle| handle.0.is_parked.load(Ordering::Acquire)),
            "All registered threads in {} must be parked before GC accesses mutators",
            self.name
        );
        state.parked.values().copied().collect()
    }

    /// Park `tid` on `caller_thread` (set its state = parked and wait for `unpark_all()`).
    pub(super) fn park(&self, tid: VMThread, caller_thread: usize) {
        let state = self.inner.lock.lock().unwrap();

        // Verify registration, active host thread ownership, and unparked state before parking.
        if let Some(&handle) = state.parked.get(&tid) {
            let expected_owner = handle.0.active_thread.load(Ordering::Acquire);
            assert_eq!(
                expected_owner, caller_thread,
                "Host thread {:#x} cannot park {:?} in {} owned by {:#x}",
                caller_thread, tid, self.name, expected_owner
            );
            assert!(
                !handle.0.is_parked.load(Ordering::Acquire),
                "Thread {:?} in {} is already parked",
                tid,
                self.name
            );
            handle.0.is_parked.store(true, Ordering::Release);
        } else {
            panic!(
                "Thread {:?} not registered to {} before park() f",
                tid, self.name
            );
        }

        // Notify any waiter that one more thread has parked
        self.inner.cvar.notify_all();

        // Wait until unpark_all() clears the parked flag of this thread. We need to check the flag,
        // as the thread may wake up spuriously, or be woken up by the notification above when
        // another thread parks. The thread cannot be unregistered while it is parked.
        let _state = self
            .inner
            .cvar
            .wait_while(state, |state| {
                state
                    .parked
                    .get(&tid)
                    .unwrap_or_else(|| {
                        panic!(
                            "Thread {:?} unregistered from {} during park()",
                            tid, self.name
                        )
                    })
                    .0
                    .is_parked
                    .load(Ordering::Acquire)
            })
            .unwrap();
    }

    /// Unpark all registered threads (wake everyone up).
    pub(super) fn unpark_all(&self) {
        let state = self.inner.lock.lock().unwrap();
        for handle in state.parked.values() {
            handle.0.is_parked.store(false, Ordering::Release);
        }
        self.inner.cvar.notify_all();
    }

    /// Block until all registered threads are parked.
    pub(super) fn wait_all_parked(&self) {
        let state = self.inner.lock.lock().unwrap();
        let _state = self
            .inner
            .cvar
            .wait_while(state, |state| {
                state.parked.is_empty()
                    || !state
                        .parked
                        .values()
                        .all(|handle| handle.0.is_parked.load(Ordering::Acquire))
            })
            .unwrap();
    }
}

/// Registry of active MMTk GC worker host threads spawned via `MockVM::spawn_gc_thread`.
pub(super) struct GcThreadSet {
    threads: Mutex<HashSet<VMThread>>,
}

impl GcThreadSet {
    pub(super) fn new() -> Self {
        Self {
            threads: Mutex::new(HashSet::new()),
        }
    }

    pub(super) fn register(&self, tid: VMThread) {
        self.threads.lock().unwrap().insert(tid);
    }

    pub(super) fn unregister(&self, tid: VMThread) {
        self.threads.lock().unwrap().remove(&tid);
    }

    pub(super) fn contains(&self, tid: VMThread) -> bool {
        self.threads.lock().unwrap().contains(&tid)
    }
}
