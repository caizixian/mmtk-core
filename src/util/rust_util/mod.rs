//! This module works around limitations of the Rust programming language, and provides missing
//! functionalities that we may expect the Rust programming language and its standard libraries
//! to provide.

pub mod atomic_box;
pub mod rev_group;
pub mod zeroed_alloc;

/// Const function for min value of two usize numbers.
pub const fn min_of_usize(a: usize, b: usize) -> usize {
    if a > b {
        b
    } else {
        a
    }
}

#[cfg(feature = "nightly")]
pub use core::intrinsics::{likely, unlikely};

// likely() and unlikely() compiler hints in stable Rust
// [1]: https://github.com/rust-lang/hashbrown/blob/a41bd76de0a53838725b997c6085e024c47a0455/src/raw/mod.rs#L48-L70
// [2]: https://users.rust-lang.org/t/compiler-hint-for-unlikely-likely-for-if-branches/62102/3
#[cfg(not(feature = "nightly"))]
#[inline]
#[cold]
fn cold() {}

#[cfg(not(feature = "nightly"))]
#[inline]
pub fn likely(b: bool) -> bool {
    if !b {
        cold();
    }
    b
}
#[cfg(not(feature = "nightly"))]
#[inline]
pub fn unlikely(b: bool) -> bool {
    if b {
        cold();
    }
    b
}

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::Once;

/// InitializeOnce creates an uninitialized value that needs to be manually initialized later. InitializeOnce
/// guarantees the value is only initialized once. This type is used to allow more efficient reads.
/// Unlike the `lazy_static!` which checks whether the static is initialized
/// in every read, InitializeOnce has no extra check for reads.
pub struct InitializeOnce<T: 'static> {
    v: UnsafeCell<MaybeUninit<T>>,
    /// This is used to guarantee `init_fn` is only called once.
    once: Once,
}

impl<T> InitializeOnce<T> {
    pub const fn new() -> Self {
        InitializeOnce {
            v: UnsafeCell::new(MaybeUninit::uninit()),
            once: Once::new(),
        }
    }

    /// Initialize the value. This should be called before ever using the struct.
    /// If this method is called by multiple threads, the first thread will
    /// initialize the value, and the other threads will be blocked until the
    /// initialization is done (`Once` returns).
    pub fn initialize_once(&self, init_fn: &'static dyn Fn() -> T) {
        self.once.call_once(|| {
            let val = init_fn();
            // SAFETY: `Once::call_once` guarantees that at most one thread executes this closure
            // to completion while blocking any concurrent `initialize_once` callers, and
            // `InitializeOnce`'s lifecycle invariant requires `initialize_once` to complete before
            // `get_ref` or `get_mut` is called. Evaluating `init_fn()` before forming `&mut` also
            // prevents re-entrant aliasing if `init_fn` were to inspect global state.
            unsafe { &mut *self.v.get() }.write(val);
        });
        debug_assert!(self.once.is_completed());
    }

    /// Get the value. This should only be used after `initialize_once()` has completed.
    pub fn get_ref(&self) -> &T {
        // We only assert in debug builds to avoid an atomic branch on hot paths (e.g. `SFT_MAP`).
        debug_assert!(self.once.is_completed());
        // SAFETY: `InitializeOnce` instances are initialized during MMTk startup via
        // `initialize_once` (enforced by `debug_assert!` above) before any thread reads them via
        // `get_ref`/`Deref`, and `Once::call_once` establishes happens-before synchronization for
        // the write to `self.v`. Any single-threaded boot-time mutation via `get_mut` finishes
        // before concurrent readers access `&self`.
        unsafe { (*self.v.get()).assume_init_ref() }
    }

    /// Get a mutable reference to the value.
    /// This is currently only used for SFTMap during plan creation (single threaded),
    /// and before the plan creation is done, the binding cannot use MMTK at all.
    ///
    /// # Safety
    /// - `self.initialize_once(...)` must have already completed before calling `get_mut`.
    /// - The caller must ensure exclusive access: no other `&T` or `&mut T` derived from `self`
    ///   may exist concurrently on this or any other thread for the lifetime of the returned
    ///   `&mut T`.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn get_mut(&self) -> &mut T {
        // We only assert in debug builds.
        debug_assert!(self.once.is_completed());
        // SAFETY: Upheld by the caller: `self.v` has been initialized by `initialize_once`, and
        // no concurrent shared or mutable references to `self.v`'s contents exist during the
        // returned mutable borrow.
        unsafe { (*self.v.get()).assume_init_mut() }
    }
}

impl<T> std::ops::Deref for InitializeOnce<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.get_ref()
    }
}

// SAFETY: Concurrent initialization of `self.v` is synchronized by `std::sync::Once`, after which
// `get_ref` only exposes shared `&T` references (while `get_mut` is `unsafe` and requires caller-
// guaranteed exclusive access). Note: all concrete types stored in static `InitializeOnce<T>` in
// `mmtk-core` (`VMLayout`, `Box<dyn SFTMap>` whose concrete impls are `Sync`, `Box<dyn VMMap>`,
// `Box<dyn Mmapper>`) are safe to share across threads; tightening the bound to `T: Send + Sync`
// requires adding `: Send + Sync` to `trait SFTMap` in Stage 2.
unsafe impl<T> Sync for InitializeOnce<T> {}

/// Create a formatted string that makes the best effort idenfying the current process and thread.
pub fn debug_process_thread_id() -> String {
    use crate::util::os::*;
    format!(
        "PID: {}, TID: {}",
        OS::get_process_id().map_or("(Failed to get PID)".to_string(), |pid| format!("{}", pid)),
        OS::get_thread_id().map_or("(Failed to get TID)".to_string(), |tid| format!(
            "{:?}",
            tid
        )),
    )
}

#[cfg(test)]
mod initialize_once_tests {
    use super::*;

    #[test]
    fn test_threads_compete_initialize() {
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;
        use std::thread;

        // Create multiple threads to initialize the same `InitializeOnce` value
        const N_THREADS: usize = 1000;
        // The test value
        static I: InitializeOnce<usize> = InitializeOnce::new();
        // Count how many times the function is called
        static INITIALIZE_COUNT: AtomicUsize = AtomicUsize::new(0);
        // The function to create initial value
        fn initialize_usize() -> usize {
            INITIALIZE_COUNT.fetch_add(1, Ordering::SeqCst);
            42
        }

        let mut threads = vec![];
        for _ in 1..N_THREADS {
            threads.push(thread::spawn(|| {
                I.initialize_once(&initialize_usize);
                // Every thread should see the value correctly initialized.
                assert_eq!(*I, 42);
            }));
        }
        threads.into_iter().for_each(|t| t.join().unwrap());

        // The initialize_usize should only be called once
        assert_eq!(INITIALIZE_COUNT.load(Ordering::SeqCst), 1);
    }
}
