//! Finite bounded index `Fin<N>` (`0 <= val < N`) for zero-`unsafe` bounds-check-free array
//! indexing (`BP 3.1`).
//!
//! Consumed by Wave 2B tracks (`s2-ms` free-list size classes, `s2-immix-malloc`, and `s2-runtime`
//! `SFTMap` / `WorkBucketStage` tables).
#![deny(unsafe_op_in_unsafe_fn)]
#![allow(dead_code)]

use core::ops::{Index, IndexMut};

/// A finite natural number strictly less than `N` (`0 <= self.get() < N`, `N > 0`).
///
/// Because `self.0 < N` is enforced at construction, indexing `[T; N]` by `Fin<N>` is 100% safe
/// (`0` `unsafe` blocks) and LLVM elides the slice bounds check whenever `N` matches the array
/// length. Note that `Fin<0>` is uninhabited (`new` and `from_pow2_mask` reject `N == 0` at
/// compile time).
#[repr(transparent)]
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub(crate) struct Fin<const N: usize>(usize);

impl<const N: usize> Fin<N> {
    /// Checked constructor returning `Some(Fin(val))` iff `val < N` (with compile-time `N > 0`
    /// check).
    #[inline(always)]
    pub(crate) const fn new(val: usize) -> Option<Self> {
        const {
            assert!(N > 0, "Fin<0> is uninhabited");
        }
        if val < N {
            Some(Self(val))
        } else {
            None
        }
    }

    /// Bitmask constructor returning `Fin(val & (N - 1))` when `N` is a compile-time power of two.
    ///
    /// Note that `usize::is_power_of_two(0)` is `false`, so `N >= 1` and `(val & (N - 1)) < N`
    /// hold unconditionally for all `val`.
    #[inline(always)]
    pub(crate) const fn from_pow2_mask(val: usize) -> Self {
        const {
            assert!(
                N.is_power_of_two(),
                "Fin::from_pow2_mask requires N to be a power of two"
            );
        }
        Self(val & (N - 1))
    }

    /// Extract the underlying `usize` guaranteed to satisfy `< N`.
    #[inline(always)]
    pub(crate) const fn get(self) -> usize {
        self.0
    }
}

impl<const N: usize> From<Fin<N>> for usize {
    #[inline(always)]
    fn from(idx: Fin<N>) -> Self {
        idx.0
    }
}

// Note: `core` provides blanket `impl<T, I, const M: usize> Index<I> for [T; M] where [T]: Index<I>`
// and `IndexMut<I> for [T; M]`, so implementing `Index<Fin<N>>` and `IndexMut<Fin<N>>` on `[T]`
// automatically enables `arr[idx]` and `arr[idx] = val` on `[T; N]` (where `self.len() == N` is
// statically known to LLVM) as well as on `[T]`.
impl<T, const N: usize> Index<Fin<N>> for [T] {
    type Output = T;

    #[inline(always)]
    fn index(&self, idx: Fin<N>) -> &Self::Output {
        // Safe slice indexing enforces `idx.0 < self.len()` at runtime and is elided by LLVM on
        // `[T; N]` whenever `Fin<N>`'s `< N` range is visible (e.g., `Fin::from_pow2_mask` or
        // `Fin::new`).
        &self[idx.0]
    }
}

impl<T, const N: usize> IndexMut<Fin<N>> for [T] {
    #[inline(always)]
    fn index_mut(&mut self, idx: Fin<N>) -> &mut Self::Output {
        &mut self[idx.0]
    }
}

#[cfg(test)]
mod tests {
    use super::Fin;

    #[test]
    fn test_fin_new_bounds_and_const_eval() {
        const ZERO: Option<Fin<4>> = Fin::<4>::new(0);
        const THREE: Option<Fin<4>> = Fin::<4>::new(3);
        const FOUR: Option<Fin<4>> = Fin::<4>::new(4);

        assert_eq!(ZERO.map(Fin::get), Some(0));
        assert_eq!(THREE.map(Fin::get), Some(3));
        assert_eq!(FOUR, None);
        assert_eq!(Fin::<4>::new(usize::MAX), None);
    }

    #[test]
    fn test_fin_from_pow2_mask() {
        const MASKED: Fin<32> = Fin::<32>::from_pow2_mask(0b11_0101);
        assert_eq!(MASKED.get(), 0b1_0101);

        for raw in [0usize, 1, 31, 32, 33, 255, usize::MAX] {
            let idx = Fin::<32>::from_pow2_mask(raw);
            assert!(idx.get() < 32);
            assert_eq!(idx.get(), raw & 31);
        }
    }

    #[test]
    fn test_fin_array_and_slice_indexing() {
        let mut arr = [10u32, 20, 30, 40];
        let i2 = Fin::<4>::new(2).unwrap();
        assert_eq!(arr[i2], 30);
        arr[i2] = 99;
        assert_eq!(arr[i2], 99);

        let slice: &mut [u32] = &mut arr[..];
        assert_eq!(slice[i2], 99);
        slice[i2] = 77;
        assert_eq!(slice[i2], 77);
    }

    #[test]
    #[should_panic(expected = "index out of bounds")]
    fn test_fin_short_slice_panics_cleanly() {
        let short_arr = [1u8, 2];
        let short_slice: &[u8] = &short_arr[..];
        let idx = Fin::<4>::new(3).unwrap();
        let _ = short_slice[idx];
    }
}
