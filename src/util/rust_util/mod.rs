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
    pub fn initialize_once<F: FnOnce() -> T>(&self, init_fn: F) {
        self.once.call_once(|| {
            // SAFETY: `Once` guarantees that this closure is called exactly once,
            // and no other threads can access the value concurrently during initialization.
            unsafe { &mut *self.v.get() }.write(init_fn());
        });
        debug_assert!(self.once.is_completed());
    }

    /// Get the value. This should only be used after initialize_once()
    pub fn get_ref(&self) -> &T {
        // We only assert in debug builds.
        debug_assert!(self.once.is_completed());
        // SAFETY: The value has been initialized by `initialize_once` (checked by debug_assert).
        // `Once` guarantees that initialization happens before any thread returns from it.
        unsafe { (*self.v.get()).assume_init_ref() }
    }


}

impl<T> std::ops::Deref for InitializeOnce<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.get_ref()
    }
}

// SAFETY: InitializeOnce uses Once to ensure that initialization happens only once.
// Once initialized, the value is never mutated. Therefore, if T is Sync, InitializeOnce<T>
// can be shared safely across threads because all accesses after initialization are read-only.
unsafe impl<T: Sync> Sync for InitializeOnce<T> {}

/// A cell that requires a proof token to access its contents mutably.
pub struct ProofCell<T> {
    value: UnsafeCell<T>,
}

impl<T> ProofCell<T> {
    pub const fn new(value: T) -> Self {
        Self {
            value: UnsafeCell::new(value),
        }
    }

    /// Get a shared reference.
    /// # Safety
    /// The caller must ensure there are no concurrent mutable accesses.
    pub unsafe fn get_ref(&self) -> &T {
        // SAFETY: The caller must ensure there are no concurrent mutable accesses.
        unsafe { &*self.value.get() }
    }

    /// Get a mutable reference with proof.
    pub fn get_mut_with_proof(&self, _proof: &crate::scheduler::ExclusivePlanAccessProof) -> &mut T {
        // SAFETY: The `ExclusivePlanAccessProof` token guarantees that we have exclusive access
        // to the plan and its components, preventing concurrent mutable access.
        unsafe { &mut *self.value.get() }
    }
}

// SAFETY: ProofCell requires a proof token for mutable access, or the caller must ensure
// no concurrent mutable access when calling the unsafe `get_ref`. Therefore, if T is Sync,
// it is safe to share ProofCell<T> across threads because concurrent reads are safe,
// and writes are restricted by proof tokens.
unsafe impl<T: Sync> Sync for ProofCell<T> {}

/// Create a formatted string that makes the best effort idenfying the current process and thread.
pub fn debug_process_thread_id() -> String {
    let pid = std::process::id();
    let tid = std::thread::current().id();
    format!("PID: {}, ThreadId: {:?}", pid, tid)
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
