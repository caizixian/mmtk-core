//! This module is for allocating large arrays or vectors with initial zero values.
//!
//! Currently we use the [`Zeroable`] trait from the `bytemuck` crate to label types that are safe
//! for zero initialization.  If one day Rust provides a standard way to optimize for zeroed
//! allocation of vectors of composite types, we can switch to the standard mechanism.
//!
//! Note: The standard library uses the `IsZero` trait to specialize the intialization of `Vec<T>`
//! if the initial element values are zero.  Primitive type, such as `i8`, `usize`, `f32`, as well
//! as types with known representations such as `Option<NonZeroUsize>` implement the `IsZero` trait.
//! However, it has several limitations.
//!
//! 1.  Composite types, such as `SpaceDescriptor(usize)`, don't implement the `IsZero` trait, even
//!     if they have the `#[repr(transparent)]` annotation.
//! 2.  The `IsZero` trait is private to the `std` module, and we cannot use it.
//!
//! Therefore, `vec![0usize; 33554432]` takes only 4 **microseconds**, while
//! `vec![SpaceDescriptor(0); 33554432]` will take 22 **milliseconds** to execute on some machine.
//! If such an allocation happens during start-up, the delay will be noticeable to light-weight
//! scripting languages, such as Ruby.
//!
//! The [`new_zeroed_vec`] function in this module can allocate zeroed vectors as fast as `vec![0;
//! LEN]`;
use std::alloc::{alloc_zeroed, handle_alloc_error, Layout};

use bytemuck::Zeroable;

/// Allocate a `Vec<T>` of all-zero values.  `T` must implement [`bytemuck::Zeroable`].
///
/// This intends to be a faster alternative to `vec![T(0), size]`.  It will allocate pre-zeroed
/// buffer, and not store zero values to its elements as part of initialization.
///
/// It is useful when creating large (hundreds of megabytes) Vecs when the execution time is
/// critical (such as during start-up, where a 100ms delay is obvious to small applications.)
///
/// Arguments:
///
/// -   `T`: The element type.
/// -   `size`: The length and capacity of the created vector.
///
/// Returns the created vector.
pub(crate) fn new_zeroed_vec<T: Zeroable>(size: usize) -> Vec<T> {
    if size == 0 || std::mem::size_of::<T>() == 0 {
        return (0..size).map(|_| T::zeroed()).collect();
    }
    let layout = Layout::array::<T>(size).unwrap();
    debug_assert!(layout.size() > 0);
    // SAFETY: `size > 0` and `size_of::<T>() > 0` were checked above, and `Layout::array`
    // succeeded without overflow, so `layout` has non-zero size and valid alignment for `T`.
    let ptr = unsafe { alloc_zeroed(layout) }.cast::<T>();
    if ptr.is_null() {
        handle_alloc_error(layout);
    }
    // SAFETY:
    // - `ptr` is non-null (`handle_alloc_error` diverges if null) and was allocated by the global
    //   allocator with exact layout `Layout::array::<T>(size)` (`size_of::<T>() > 0`).
    // - `alloc_zeroed` initialized all `size * size_of::<T>()` bytes to zero, and `T: Zeroable`
    //   guarantees that all-zero bytes form valid initialized values of `T` for all `size` items.
    // - `length == capacity == size`, and `Layout::array::<T>(size)` verified total byte size
    //   `<= isize::MAX`.
    unsafe { Vec::from_raw_parts(ptr, size, size) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_zeroed_vec_empty_and_zst() {
        let empty: Vec<u64> = new_zeroed_vec(0);
        assert!(empty.is_empty());

        let zst: Vec<()> = new_zeroed_vec(16);
        assert_eq!(zst.len(), 16);

        let non_empty: Vec<usize> = new_zeroed_vec(128);
        assert_eq!(non_empty.len(), 128);
        assert!(non_empty.iter().all(|&x| x == 0));
    }
}
