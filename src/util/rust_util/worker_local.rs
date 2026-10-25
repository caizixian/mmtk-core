//! Instance-branded per-worker permissions (`WorkerPerm`) and false-sharing-padded worker-local
//! storage (`WorkerLocalSlice<T>`, `BP 4.2`).
//!
//! Consumed by Wave 2B (`s2-runtime` in `scheduler/worker.rs`, `lxr/mature_evac.rs`, and `lisp2`).
#![deny(unsafe_op_in_unsafe_fn)]
#![allow(dead_code)]

use super::phase_cell::{MmtkId, WorkersParkedToken};
use crossbeam::utils::CachePadded;
use std::cell::UnsafeCell;
use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::sync::Mutex;

/// Registry recording the exact worker count minted for each `MmtkId`, ensuring `WorkerPerm` sets
/// can be minted **at most once** per `MmtkId`.
static MINTED_WORKER_COUNTS: Mutex<BTreeMap<NonZeroU64, usize>> = Mutex::new(BTreeMap::new());

/// Affine (`!Copy`, `!Clone`) capability token owned by a single GC worker (`ordinal` within a
/// specific [`MmtkId`] instance).
///
/// Holding `&mut WorkerPerm` proves unique access to slot `ordinal` across every
/// [`WorkerLocalSlice<T>`] branded with `self.instance_id`.
#[derive(Debug)]
pub(crate) struct WorkerPerm {
    instance_id: MmtkId,
    ordinal: usize,
}

impl WorkerPerm {
    /// Pre-mint the complete set of `0..num_workers` [`WorkerPerm`] tokens for `instance_id`
    /// (called once by `WorkerGroup::new` during `MMTK` creation).
    ///
    /// # Panics
    ///
    /// Panics if `mint_for_instance` has already been called for `instance_id`.
    pub(crate) fn mint_for_instance(instance_id: MmtkId, num_workers: usize) -> Vec<Self> {
        let prev = MINTED_WORKER_COUNTS
            .lock()
            .unwrap()
            .insert(instance_id.as_nonzero(), num_workers);
        assert!(
            prev.is_none(),
            "WorkerPerm::mint_for_instance called more than once for {instance_id:?}"
        );
        (0..num_workers)
            .map(|ordinal| Self {
                instance_id,
                ordinal,
            })
            .collect()
    }

    /// Return the [`MmtkId`] brand of the [`MMTK`](crate::MMTK) instance owning this worker.
    #[inline(always)]
    pub(crate) const fn instance_id(&self) -> MmtkId {
        self.instance_id
    }

    /// Return this worker's `0`-based ordinal index.
    #[inline(always)]
    pub(crate) const fn ordinal(&self) -> usize {
        self.ordinal
    }
}

impl<'a> WorkersParkedToken<'a> {
    /// Safely mint a [`WorkersParkedToken<'a>`] by mutably borrowing the complete slice of all
    /// `WorkerPerm`s ever minted for `instance_id` (requires `num_workers > 0`).
    ///
    /// Because [`WorkerPerm::mint_for_instance`] mints at most `N` `WorkerPerm` values with
    /// `instance_id` and `perms: &'a mut [WorkerPerm]` holds `N >= 1` mutually disjoint `&'a mut`
    /// borrows all matching `instance_id`, the pigeonhole principle guarantees that **every**
    /// `WorkerPerm` for `instance_id` is borrowed in `perms` for the entire lifetime `'a` (so none
    /// can be concurrently used in [`WorkerLocalSlice::get_mut`], in a second `from_all_perms`
    /// call, or inside `WorkerMonitor::park_and_wait`).
    pub(crate) fn from_all_perms(instance_id: MmtkId, perms: &'a mut [WorkerPerm]) -> Self {
        let expected_count = *MINTED_WORKER_COUNTS
            .lock()
            .unwrap()
            .get(&instance_id.as_nonzero())
            .expect("WorkersParkedToken::from_all_perms: no WorkerPerms minted for instance_id");
        assert!(
            expected_count > 0,
            "WorkersParkedToken::from_all_perms requires at least 1 worker"
        );
        assert_eq!(
            perms.len(),
            expected_count,
            "WorkersParkedToken::from_all_perms requires all {expected_count} WorkerPerms, got {}",
            perms.len()
        );
        for perm in perms.iter() {
            assert_eq!(
                perm.instance_id(),
                instance_id,
                "WorkersParkedToken::from_all_perms: mismatched WorkerPerm instance_id"
            );
        }
        // SAFETY: `mint_for_instance` minted exactly `expected_count >= 1` `WorkerPerm`s for
        // `instance_id`, and `perms` mutably borrows `expected_count` distinct `WorkerPerm`s all
        // carrying `instance_id` for `'a`. By the pigeonhole principle, all `WorkerPerm`s of
        // `instance_id` are quiescent on the current thread (establishing trivial happens-before)
        // and cannot be passed to `get_mut`, another `from_all_perms`, or `WorkerMonitor` for `'a`.
        unsafe { Self::new_unchecked(instance_id) }
    }
}

/// Cache-line-padded per-worker storage indexed safely via [`WorkerPerm`] during parallel phases
/// or drained collectively via [`WorkersParkedToken`] when all workers are parked.
pub(crate) struct WorkerLocalSlice<T> {
    instance_id: MmtkId,
    slots: Box<[CachePadded<UnsafeCell<T>>]>,
}

// SAFETY: Dropping or moving `WorkerLocalSlice<T>` exclusively moves/drops each `T` slot, which is
// sound whenever `T: Send`.
unsafe impl<T: Send> Send for WorkerLocalSlice<T> {}

// SAFETY: `WorkerLocalSlice<T>` never hands out shared `&T` references across threads; `&self`
// only yields exclusive `&mut T` references under two mutually exclusive regimes:
// 1. `get_mut(&self, perm: &mut WorkerPerm)`: `WorkerPerm::mint_for_instance` mints at most one
//    `WorkerPerm` per `(instance_id, ordinal)`, so two workers never access the same `ordinal`
//    concurrently, and transferring a slot's `&mut T` to the worker thread owning `perm` is sound
//    because `T: Send`.
// 2. `iter_mut_all(&self, parked: &mut WorkersParkedToken)`: `WorkersParkedToken` witnesses that
//    all workers for `instance_id` are parked (with `Release`/`Acquire` synchronization from every
//    worker's prior `get_mut` accesses) and `&mut parked` prevents concurrent `iter_mut_all` or
//    `get_mut` calls during the borrow, so handing `&mut T` for every slot to the caller thread is
//    sound because `T: Send` (matching `std::sync::Mutex<T>`'s `T: Send` bound for `Sync`).
unsafe impl<T: Send> Sync for WorkerLocalSlice<T> {}

impl<T> WorkerLocalSlice<T> {
    /// Construct a `WorkerLocalSlice<T>` of length `len` for `instance_id`, initializing each
    /// worker slot `0..len` with `init(ordinal)`.
    pub(crate) fn from_fn(
        instance_id: MmtkId,
        len: usize,
        mut init: impl FnMut(usize) -> T,
    ) -> Self {
        let slots: Vec<CachePadded<UnsafeCell<T>>> = (0..len)
            .map(|ordinal| CachePadded::new(UnsafeCell::new(init(ordinal))))
            .collect();
        Self {
            instance_id,
            slots: slots.into_boxed_slice(),
        }
    }

    /// Return the [`MmtkId`] brand of this slice.
    #[inline(always)]
    pub(crate) const fn instance_id(&self) -> MmtkId {
        self.instance_id
    }

    /// Return the number of worker slots in this slice.
    #[inline(always)]
    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    /// Return `true` if this slice has zero slots.
    #[inline(always)]
    pub(crate) fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Borrow the calling worker's exclusive `&mut T` slot using its `&mut WorkerPerm`.
    ///
    /// # Panics
    ///
    /// Panics if `self.instance_id != perm.instance_id` or if `perm.ordinal >= self.len()`.
    #[inline(always)]
    pub(crate) fn get_mut<'a>(&'a self, perm: &'a mut WorkerPerm) -> &'a mut T {
        assert_eq!(
            self.instance_id, perm.instance_id,
            "WorkerLocalSlice::get_mut MmtkId mismatch"
        );
        let slot: &'a CachePadded<UnsafeCell<T>> = &self.slots[perm.ordinal];
        // SAFETY: `WorkerPerm::mint_for_instance` mints at most one `WorkerPerm` for
        // `(self.instance_id, perm.ordinal)`, `perm` is mutably borrowed for `'a` (preventing
        // reentrant `get_mut` on `self` with the same `perm`), and `WorkersParkedToken`'s invariant
        // guarantees no `WorkersParkedToken` for `self.instance_id` is live while `perm` is active
        // (preventing concurrent `iter_mut_all`). Note that two distinct `WorkerLocalSlice`s may
        // not be borrowed with the same `&mut WorkerPerm` simultaneously because `perm` is mutably
        // borrowed for `'a`, which also prevents aliasing if two `&WorkerLocalSlice<T>` references
        // point to the same slice!
        unsafe { &mut *slot.get() }
    }

    /// Mutably iterate over all worker slots while all workers for `self.instance_id` are parked.
    ///
    /// # Panics
    ///
    /// Panics if `self.instance_id != parked.instance_id()`.
    #[inline]
    pub(crate) fn iter_mut_all<'a>(
        &'a self,
        parked: &'a mut WorkersParkedToken<'_>,
    ) -> impl ExactSizeIterator<Item = &'a mut T> {
        assert_eq!(
            self.instance_id,
            parked.instance_id(),
            "WorkerLocalSlice::iter_mut_all MmtkId mismatch"
        );
        self.slots.iter().map(|slot| {
            // SAFETY: `parked` witnesses that all `WorkerPerm`s for `self.instance_id` are parked
            // (with happens-before synchronization from prior worker accesses) and no other
            // `WorkersParkedToken` exists for `self.instance_id`. Borrowing `parked` mutably for
            // `'a` prevents any second `iter_mut_all` call (including via an alias `&self`) during
            // `'a`, and `self.slots.iter()` visits each distinct `UnsafeCell<T>` at most once.
            unsafe { &mut *slot.get() }
        })
    }

    /// Mutably iterate over all worker slots when holding exclusive `&mut self` (0 `unsafe`).
    #[inline]
    pub(crate) fn get_mut_exclusive(&mut self) -> impl ExactSizeIterator<Item = &mut T> {
        self.slots.iter_mut().map(|slot| slot.get_mut())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use std::cell::Cell;
    use std::rc::Rc;
    use std::thread;

    assert_impl_all!(WorkerPerm: Send, Sync);
    assert_not_impl_any!(WorkerPerm: Copy, Clone);
    assert_impl_all!(WorkerLocalSlice<Vec<usize>>: Send, Sync);
    // `Cell<usize>` is `Send + !Sync`, and `WorkerLocalSlice` only grants serialized `&mut T`,
    // never concurrent `&T`, so `WorkerLocalSlice<Cell<usize>>` is `Send + Sync`.
    assert_impl_all!(WorkerLocalSlice<Cell<usize>>: Send, Sync);
    assert_not_impl_any!(WorkerLocalSlice<Rc<usize>>: Send, Sync);

    #[test]
    fn test_worker_local_parallel_and_parked_drain() {
        let id = MmtkId::fresh();
        let mut perms = WorkerPerm::mint_for_instance(id, 4);
        let mut locals = WorkerLocalSlice::from_fn(id, 4, |ord| vec![ord]);

        for (i, slot) in locals.get_mut_exclusive().enumerate() {
            assert_eq!(slot.as_slice(), &[i]);
        }

        thread::scope(|s| {
            for perm in &mut perms {
                let locals_ref = &locals;
                s.spawn(move || {
                    let ord = perm.ordinal();
                    locals_ref.get_mut(perm).push(ord * 10);
                });
            }
        });

        let mut parked = WorkersParkedToken::from_all_perms(id, &mut perms);
        let collected: Vec<Vec<usize>> = locals
            .iter_mut_all(&mut parked)
            .map(std::mem::take)
            .collect();
        assert_eq!(
            collected,
            vec![vec![0, 0], vec![1, 10], vec![2, 20], vec![3, 30]]
        );
    }

    #[test]
    #[should_panic(expected = "WorkerLocalSlice::get_mut MmtkId mismatch")]
    fn test_worker_local_cross_instance_perm_panics() {
        let id1 = MmtkId::fresh();
        let id2 = MmtkId::fresh();
        let locals1 = WorkerLocalSlice::from_fn(id1, 2, |_| 0usize);
        let mut perms2 = WorkerPerm::mint_for_instance(id2, 2);

        let _ = locals1.get_mut(&mut perms2[0]);
    }

    #[test]
    #[should_panic(expected = "WorkerLocalSlice::iter_mut_all MmtkId mismatch")]
    fn test_worker_local_cross_instance_parked_panics() {
        let id1 = MmtkId::fresh();
        let id2 = MmtkId::fresh();
        let locals1 = WorkerLocalSlice::from_fn(id1, 2, |_| 0usize);
        let mut perms2 = WorkerPerm::mint_for_instance(id2, 2);
        let mut parked2 = WorkersParkedToken::from_all_perms(id2, &mut perms2);

        let _ = locals1.iter_mut_all(&mut parked2);
    }

    #[test]
    #[should_panic(expected = "index out of bounds")]
    fn test_worker_local_ordinal_out_of_bounds_panics() {
        let id = MmtkId::fresh();
        let mut perms = WorkerPerm::mint_for_instance(id, 4);
        let shorter = WorkerLocalSlice::from_fn(id, 2, |_| 0usize);

        let _ = shorter.get_mut(&mut perms[3]);
    }

    #[test]
    #[should_panic(expected = "WorkerPerm::mint_for_instance called more than once")]
    fn test_duplicate_worker_perm_mint_panics() {
        let id = MmtkId::fresh();
        let _p1 = WorkerPerm::mint_for_instance(id, 2);
        let _p2 = WorkerPerm::mint_for_instance(id, 2);
    }
}
