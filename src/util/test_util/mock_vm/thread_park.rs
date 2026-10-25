use crate::util::VMThread;
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};

#[derive(Clone)]
pub(crate) struct ThreadPark {
    name: &'static str,
    inner: Arc<Inner>,
}

struct Inner {
    lock: Mutex<State>,
    cvar: Condvar,
}

struct ThreadEntry {
    owner_thread: VMThread,
    is_parked: bool,
}

#[derive(Default)]
struct State {
    /// All registered threads, their active owning host thread token, and whether they are parked.
    parked: HashMap<VMThread, ThreadEntry>,
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

    /// Register `tid` with initial owning host thread `owner_thread`.
    pub(super) fn register(&self, tid: VMThread, owner_thread: VMThread) {
        debug!("Register {:?} to {}", tid, self.name);
        let mut state = self.inner.lock.lock().unwrap();
        state.parked.insert(
            tid,
            ThreadEntry {
                owner_thread,
                is_parked: false,
            },
        );
    }

    pub(super) fn unregister(&self, tid: VMThread) {
        let mut state = self.inner.lock.lock().unwrap();
        state.parked.remove(&tid);
    }

    pub(super) fn contains(&self, tid: VMThread) -> bool {
        let state = self.inner.lock.lock().unwrap();
        state.parked.contains_key(&tid)
    }

    /// Verify `tid` is registered and currently NOT parked at a safepoint, and atomically record
    /// `caller_thread` as the active host thread allowed to park `tid` in `block_for_gc`.
    pub(crate) fn claim_unparked(&self, tid: VMThread, caller_thread: VMThread) {
        let mut state = self.inner.lock.lock().unwrap();
        let entry = state.parked.get_mut(&tid).unwrap_or_else(|| {
            panic!("Thread {:?} is not registered in {}", tid, self.name);
        });
        assert!(
            !entry.is_parked,
            "Cannot borrow fixture mutator {:?} while it is parked in {}",
            tid, self.name
        );
        entry.owner_thread = caller_thread;
    }

    pub(super) fn number_of_threads(&self) -> usize {
        let state = self.inner.lock.lock().unwrap();
        state.parked.len()
    }

    /// Return all registered threads, asserting that every registered thread is currently parked.
    pub(super) fn all_parked_threads(&self) -> Vec<VMThread> {
        let state = self.inner.lock.lock().unwrap();
        assert!(
            state.parked.values().all(|entry| entry.is_parked),
            "All registered threads in {} must be parked before GC accesses mutators",
            self.name
        );
        state.parked.keys().copied().collect()
    }

    /// Park `tid` on `caller_thread` (set its state = parked and wait for `unpark_all()`).
    pub(super) fn park(&self, tid: VMThread, caller_thread: VMThread) {
        let mut state = self.inner.lock.lock().unwrap();

        // Verify registration, active host thread ownership, and unparked state before parking.
        if let Some(entry) = state.parked.get_mut(&tid) {
            assert_eq!(
                entry.owner_thread, caller_thread,
                "Host thread {:?} cannot park {:?} in {} owned by {:?}",
                caller_thread, tid, self.name, entry.owner_thread
            );
            assert!(
                !entry.is_parked,
                "Thread {:?} in {} is already parked",
                tid, self.name
            );
            entry.is_parked = true;
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
                    .is_parked
            })
            .unwrap();
    }

    /// Unpark all registered threads (wake everyone up).
    pub(super) fn unpark_all(&self) {
        let mut state = self.inner.lock.lock().unwrap();
        for entry in state.parked.values_mut() {
            entry.is_parked = false;
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
                state.parked.is_empty() || !state.parked.values().all(|entry| entry.is_parked)
            })
            .unwrap();
    }
}
