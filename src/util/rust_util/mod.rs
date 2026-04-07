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
use std::sync::OnceLock;

/// InitializeOnce creates an uninitialized value that needs to be manually initialized later.
/// InitializeOnce guarantees the value is only initialized once.
///
/// Internally uses `OnceLock` for thread-safe one-time initialization, wrapped in `UnsafeCell`
/// to allow the `get_mut` path during single-threaded plan creation.
pub struct InitializeOnce<T: 'static> {
    lock: UnsafeCell<OnceLock<T>>,
}

impl<T> InitializeOnce<T> {
    pub const fn new() -> Self {
        InitializeOnce {
            lock: UnsafeCell::new(OnceLock::new()),
        }
    }

    fn get_lock(&self) -> &OnceLock<T> {
        // SAFETY: We only read the OnceLock through shared references.
        // OnceLock's own synchronization handles thread-safe initialization.
        unsafe { &*self.lock.get() }
    }

    /// Initialize the value. This should be called before ever using the struct.
    /// If this method is called by multiple threads, the first thread will
    /// initialize the value, and the other threads will be blocked until the
    /// initialization is done.
    pub fn initialize_once(&self, init_fn: &'static dyn Fn() -> T) {
        self.get_lock().get_or_init(init_fn);
    }

    /// Get the value. This should only be used after initialize_once()
    pub fn get_ref(&self) -> &T {
        self.get_lock().get().expect("InitializeOnce is not yet initialized")
    }

    /// Get a mutable reference to the value.
    /// This is currently only used for SFTMap during plan creation (single threaded),
    /// and before the plan creation is done, the binding cannot use MMTK at all.
    ///
    /// # Safety
    /// The caller needs to make sure there is no race when mutating the value.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn get_mut(&self) -> &mut T {
        // SAFETY: UnsafeCell allows interior mutability. The caller guarantees
        // no concurrent access. OnceLock::get_mut requires &mut self, but we
        // bypass this because we have a stronger guarantee (single-threaded phase).
        let lock = unsafe { &mut *self.lock.get() };
        lock.get_mut().expect("InitializeOnce is not yet initialized")
    }
}

impl<T> std::ops::Deref for InitializeOnce<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.get_ref()
    }
}

// SAFETY: InitializeOnce is safe to share between threads. OnceLock ensures
// initialization is thread-safe. After initialization, the value is only
// read (via get_ref/Deref) or mutated via unsafe get_mut which requires the
// caller to ensure no races. The unsafe impl is needed because some T (e.g.,
// Box<dyn SFTMap>) don't implement Sync, but we only store them once and
// read immutably afterwards.
unsafe impl<T> Sync for InitializeOnce<T> {}

/// Create a formatted string that makes the best effort identifying the current process and thread.
pub fn debug_process_thread_id() -> String {
    let pid = std::process::id();
    #[cfg(target_os = "linux")]
    {
        let tid = std::thread::current().id();
        format!("PID: {}, TID: {:?}", pid, tid)
    }
    #[cfg(not(target_os = "linux"))]
    {
        format!("PID: {}", pid)
    }
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
