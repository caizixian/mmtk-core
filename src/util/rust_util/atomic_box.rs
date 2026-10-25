use std::marker::PhantomData;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::OnceLock;

use bytemuck::Zeroable;

const fn at_least_acquire(order: Ordering) -> Ordering {
    match order {
        Ordering::Relaxed | Ordering::Release | Ordering::Acquire | Ordering::AcqRel => {
            Ordering::Acquire
        }
        Ordering::SeqCst => Ordering::SeqCst,
        _ => Ordering::SeqCst,
    }
}

const fn at_least_release(order: Ordering) -> Ordering {
    match order {
        Ordering::Relaxed | Ordering::Release => Ordering::Release,
        Ordering::Acquire | Ordering::AcqRel => Ordering::AcqRel,
        Ordering::SeqCst => Ordering::SeqCst,
        _ => Ordering::SeqCst,
    }
}

/// A lazily initialized box.  Similar to an `Option<Box<T>>`, but can be initialized atomically.
///
/// It is designed for implementing shared data.  Therefore, methods with `&self`, namely
/// [`OnceOptionBox::get`] and the [`OnceOptionBox::get_or_init`] methods, only return shared
/// references to the content (`&T`).  The user should use types that support multi-threaded
/// accesses, such as mutexes or atomic types, if the inner type is supposed to be modified
/// concurrently.
///
/// Once initialized, this object will own its content.  The content is allocated in the heap, and
/// will be dropped and deallocated when this instance is dropped.
///
/// # Comparison to existing data structures
///
/// [`std::sync::OnceLock`] also provides thread-safe lazily-initialized cells.  But as its name
/// suggests, it uses locks for synchronization, whereas `OnceOptionBox` is lock-free.  `OnceLock`
/// also has a field of [`std::sync::Once`] which increases the space overhead.  `OnceOptionBox`
/// only has one atomic pointer field and is more suitable for large arrays of lazily initialized
/// elements.
#[repr(transparent)]
pub struct OnceOptionBox<T> {
    inner: AtomicPtr<T>,
    // Constrains `Send` and `Sync` auto-traits identically to `OnceLock<T>`:
    // `OnceOptionBox<T>: Send` iff `T: Send`, and `OnceOptionBox<T>: Sync` iff `T: Send + Sync`
    // (as `get_or_init` may allocate `Box<T>` on one thread and drop it on another, while `get`
    // yields `&T` across threads).
    _marker: PhantomData<OnceLock<T>>,
}

impl<T> OnceOptionBox<T> {
    /// Create an empty `OnceOptionBox` instance.
    pub fn new() -> OnceOptionBox<T> {
        Self {
            inner: AtomicPtr::new(std::ptr::null_mut()),
            _marker: PhantomData,
        }
    }

    /// Get a reference to the content of this box, or `None` if not yet initialized.
    pub fn get(&self, order: Ordering) -> Option<&T> {
        // Use at least `Acquire` ordering so the `Release` store in `get_or_init` synchronizes-with
        // this load and the initialized heap allocation is fully visible before dereferencing.
        let ptr = self.inner.load(at_least_acquire(order));
        // SAFETY: `self.inner` is initialized to null (in `new` or via `Zeroable`), transitions at
        // most once from null to a valid `Box::into_raw` pointer using at least `Release` ordering
        // in `get_or_init`, and is never mutated or deallocated until `Drop::drop(&mut self)`.
        // Thus if `ptr` is non-null, it is properly aligned, dereferenceable, initialized, and
        // only shared references `&T` exist for `&self`'s lifetime.
        unsafe { ptr.as_ref() }
    }

    /// Get a reference to the content of this box.  If not initialized, it will call `init` to
    /// initialize this box.
    ///
    /// When multiple threads attempt to initialize this box concurrently, all threads may call
    /// their supplied `init` closure, but only one thread will successfully initialize this box to
    /// the return value of `init`.  Other threads will drop their return values of `init`.  All
    /// callers will return the reference to the value created by the successful thread.
    pub fn get_or_init(
        &self,
        order_load: Ordering,
        order_store: Ordering,
        init: impl FnOnce() -> T,
    ) -> &T {
        if let Some(get_result) = self.get(order_load) {
            return get_result;
        }

        let new_inner = Box::into_raw(Box::new(init()));
        let cas_result = self.inner.compare_exchange(
            std::ptr::null_mut(),
            new_inner,
            at_least_release(order_store),
            at_least_acquire(order_load),
        );
        match cas_result {
            Ok(old_inner) => {
                debug_assert_eq!(old_inner, std::ptr::null_mut());
                // SAFETY: `new_inner` came from `Box::into_raw` above and was just published into
                // `self.inner`, which only ever hands out shared `&T` references until `Drop` has
                // exclusive `&mut self`.
                unsafe { &*new_inner }
            }
            Err(old_inner) => {
                // SAFETY: `compare_exchange` failed, so `new_inner` was never shared and remains
                // uniquely owned by this thread as the raw pointer from `Box::into_raw` above.
                drop(unsafe { Box::from_raw(new_inner) });
                debug_assert!(!old_inner.is_null());
                // SAFETY: Because `compare_exchange` expected `null_mut()` and failed with at
                // least `Acquire` ordering, `old_inner` is the non-null `Box::into_raw` pointer
                // published by the winning thread with at least `Release` ordering; it remains
                // valid, initialized, and immutable (aside from interior mutability on `T: Sync`)
                // for the lifetime of `&self`.
                unsafe { &*old_inner }
            }
        }
    }
}

impl<T> Drop for OnceOptionBox<T> {
    fn drop(&mut self) {
        let ptr = *self.inner.get_mut();
        if !ptr.is_null() {
            // SAFETY: `&mut self` guarantees exclusive access with no live `&T` borrows, and any
            // non-null `ptr` in `self.inner` was created once via `Box::into_raw(Box::new(...))`
            // in `get_or_init` and has not yet been freed.
            drop(unsafe { Box::from_raw(ptr) });
        }
    }
}

// SAFETY: `OnceOptionBox<T>` is `#[repr(transparent)]` around `AtomicPtr<T>` (plus zero-sized
// `PhantomData`). `AtomicPtr<T>` is guaranteed to have the same layout and bit validity as
// `*mut T`, for which all-zero bytes represent a valid null pointer (`OnceOptionBox` uninitialized
// state).
unsafe impl<T> Zeroable for OnceOptionBox<T> {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    static_assertions::assert_impl_all!(OnceOptionBox<usize>: Send, Sync);
    static_assertions::assert_impl_all!(OnceOptionBox<Cell<usize>>: Send);
    static_assertions::assert_not_impl_any!(OnceOptionBox<Cell<usize>>: Sync);
    static_assertions::assert_not_impl_any!(OnceOptionBox<*mut u8>: Send, Sync);

    #[test]
    fn construct() {
        let oob = OnceOptionBox::<usize>::new();
        let elem = oob.get(Ordering::Relaxed);
        assert_eq!(elem, None);
    }

    #[test]
    fn init() {
        let oob = OnceOptionBox::<usize>::new();
        let elem = oob.get_or_init(Ordering::Relaxed, Ordering::Relaxed, || 42);
        assert_eq!(*elem, 42);
    }

    #[test]
    fn reinit() {
        let oob = OnceOptionBox::<usize>::new();
        let elem = oob.get_or_init(Ordering::Relaxed, Ordering::Relaxed, || 42);
        assert_eq!(*elem, 42);
        let elem2 = oob.get_or_init(Ordering::Relaxed, Ordering::Relaxed, || 43);
        assert_eq!(*elem2, 42);
    }
}
