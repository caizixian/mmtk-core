use std::sync::atomic::{AtomicPtr, Ordering};

use bytemuck::Zeroable;

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
pub struct OnceOptionBox<T> {
    inner: AtomicPtr<T>,
}

impl<T> OnceOptionBox<T> {
    /// Create an empty `OnceOptionBox` instance.
    pub fn new() -> OnceOptionBox<T> {
        Self {
            inner: AtomicPtr::new(std::ptr::null_mut()),
        }
    }

    /// Get a reference to the content of this box, or `None` if not yet initialized.
    pub fn get(&self, order: Ordering) -> Option<&T> {
        let ptr = self.inner.load(order);
        // SAFETY: If `ptr` is not null, it points to a valid `T` allocated by `Box::new`
        // and owned by this `OnceOptionBox`.
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
            order_store,
            Ordering::Relaxed,
        );
        match cas_result {
            Ok(old_inner) => {
                debug_assert_eq!(old_inner, std::ptr::null_mut());
                // SAFETY: `new_inner` was created from a `Box` and is valid.
                unsafe { new_inner.as_ref().unwrap() }
            }
            Err(old_inner) => {
                // SAFETY: We failed to store `new_inner`, so we take ownership back to drop it.
                drop(unsafe { Box::from_raw(new_inner) });
                // SAFETY: `old_inner` was successfully stored by another thread and is valid.
                unsafe { old_inner.as_ref().unwrap() }
            }
        }
    }
}

impl<T> Drop for OnceOptionBox<T> {
    fn drop(&mut self) {
        let ptr = *self.inner.get_mut();
        if !ptr.is_null() {
            // SAFETY: The pointer was allocated by `Box::new` and we are dropping it.
            drop(unsafe { Box::from_raw(ptr) });
        }
    }
}

// SAFETY: `OnceOptionBox` contains only an `AtomicPtr`. A zeroed `AtomicPtr` represents
// a null pointer, which is the valid representation of an empty `OnceOptionBox`.
unsafe impl<T> Zeroable for OnceOptionBox<T> {}

#[cfg(test)]
mod tests {
    use super::*;

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
