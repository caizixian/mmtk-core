//! Instance-branded stop-the-world phase capabilities and interior-mutability cells (`BP 4.1`,
//! `BP 4.2`).
//!
//! Inspired by `GhostCell` (Yanovski et al., ICFP '21), `QCell`, and Rust-for-Linux `LockedBy`:
//! every [`MMTK`](crate::MMTK) instance mints a unique monotonic [`MmtkId`] at startup, and its
//! [`PlanPhaseAuth`] + [`StwCell<T>`] cells share that runtime brand. Mutating an [`StwCell<T>`]
//! through a shared `&StwCell<T>` reference requires `&mut PlanPhaseLease<'_>`, which can only be
//! obtained from [`PlanPhaseAuth::with_exclusive`] when holding a [`WorkersParkedToken`] for the
//! matching `MmtkId`.
//!
//! Consumed by Wave 2B (`s2-runtime` in `mmtk.rs`, `scheduler`, and `plan`, plus `s2-ms` /
//! `s2-immix-malloc` space phase state).
#![deny(unsafe_op_in_unsafe_fn)]
#![allow(dead_code)]

use std::cell::UnsafeCell;
use std::collections::BTreeSet;
use std::marker::PhantomData;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

/// Unique runtime brand identifying a single [`MMTK`](crate::MMTK) instance.
///
/// Using a 64-bit monotonic counter (`AtomicU64` starting at `1`) with an explicit overflow
/// assertion ensures `MmtkId` values are never recycled for the lifetime of the OS process, even
/// in multi-instance test suites.
#[repr(transparent)]
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub(crate) struct MmtkId(NonZeroU64);

impl MmtkId {
    /// Mint a fresh, process-unique `MmtkId`.
    #[inline]
    pub(crate) fn fresh() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        assert!(
            id != 0 && id < u64::MAX / 2,
            "MmtkId monotonic counter exhausted"
        );
        Self(NonZeroU64::new(id).unwrap())
    }

    /// Return the underlying `NonZeroU64` brand value.
    #[inline(always)]
    pub(crate) const fn as_nonzero(self) -> NonZeroU64 {
        self.0
    }

    /// Return the underlying raw `u64` brand value.
    #[inline(always)]
    pub(crate) const fn as_u64(self) -> u64 {
        self.0.get()
    }
}

/// Phantom marker making a token `!Send`, `!Sync`, and **invariant** in `'a`.
///
/// - `*mut ()` opts out of both `Send` and `Sync`.
/// - `fn(&'a ()) -> &'a ()` places `'a` in both contravariant and covariant positions, making `'a`
///   strictly invariant (note that `&'a mut ()` alone is covariant in `'a`, not invariant).
type InvariantUnsendUnsync<'a> = PhantomData<(*mut (), fn(&'a ()) -> &'a ())>;

/// Capability token witnessing that all GC workers belonging to `instance_id` are parked (with
/// `Release`/`Acquire` happens-before synchronization from every worker's prior accesses to the
/// current thread) and that no other `WorkersParkedToken` is active for `instance_id` during `'a`.
///
/// `!Copy`, `!Clone`, `!Send`, `!Sync`, and invariant in `'a`.
pub(crate) struct WorkersParkedToken<'a> {
    instance_id: MmtkId,
    _marker: InvariantUnsendUnsync<'a>,
}

impl<'a> WorkersParkedToken<'a> {
    /// Mint a `WorkersParkedToken<'a>` once the scheduler's `WorkerMonitor` has verified that all
    /// workers for `instance_id` are parked under the monitor mutex.
    ///
    /// # Safety
    ///
    /// For the entire lifetime `'a`:
    /// 1. Every `WorkerPerm` minted for `instance_id` must be quiescent/parked (none may be
    ///    concurrently passed to [`super::worker_local::WorkerLocalSlice::get_mut`] or
    ///    [`WorkersParkedToken::from_all_perms`]), and all prior memory accesses performed by
    ///    those workers must happen-before (`Acquire`/`Release` or stronger, e.g., via the
    ///    `WorkerMonitor` mutex) this call; and
    /// 2. No other live `WorkersParkedToken` for `instance_id` may exist concurrently during `'a`,
    ///    and workers must not be unparked until `'a` has ended.
    #[inline(always)]
    pub(crate) unsafe fn new_unchecked(instance_id: MmtkId) -> Self {
        Self {
            instance_id,
            _marker: PhantomData,
        }
    }

    /// Return the [`MmtkId`] instance brand witnessed by this token.
    #[inline(always)]
    pub(crate) const fn instance_id(&self) -> MmtkId {
        self.instance_id
    }

    /// Temporarily reborrow `self` with a shorter lifetime tied to `&mut self`.
    ///
    /// Because `self` is mutably borrowed for the returned token's lifetime, `self` cannot be used
    /// concurrently until the reborrowed token drops.
    #[inline(always)]
    pub(crate) fn reborrow(&mut self) -> WorkersParkedToken<'_> {
        WorkersParkedToken {
            instance_id: self.instance_id,
            _marker: PhantomData,
        }
    }
}

/// Per-instance authority that mints [`PlanPhaseLease`] exclusively while all workers are parked.
///
/// At most one `PlanPhaseAuth` may ever be constructed for a given [`MmtkId`], preventing two
/// authorities with the same `instance_id` from ever existing.
pub(crate) struct PlanPhaseAuth {
    instance_id: MmtkId,
    leased: AtomicBool,
}

impl PlanPhaseAuth {
    /// Construct the unique `PlanPhaseAuth` for `instance_id`.
    ///
    /// # Panics
    ///
    /// Panics if a `PlanPhaseAuth` has already been constructed for `instance_id`.
    pub(crate) fn new(instance_id: MmtkId) -> Self {
        static REGISTERED_AUTHS: Mutex<BTreeSet<NonZeroU64>> = Mutex::new(BTreeSet::new());
        let inserted = REGISTERED_AUTHS
            .lock()
            .unwrap()
            .insert(instance_id.as_nonzero());
        assert!(
            inserted,
            "PlanPhaseAuth::new called more than once for {instance_id:?}"
        );
        Self {
            instance_id,
            leased: AtomicBool::new(false),
        }
    }

    /// Return the [`MmtkId`] instance brand governed by this authority.
    #[inline(always)]
    pub(crate) const fn instance_id(&self) -> MmtkId {
        self.instance_id
    }

    /// Execute `f` with an exclusive [`PlanPhaseLease`] while all workers for `self.instance_id`
    /// are parked.
    ///
    /// An RAII drop guard releases `self.leased` even if `f` panics and unwinds (`#29.1`).
    ///
    /// # Panics
    ///
    /// Panics if `parked.instance_id() != self.instance_id` or if `with_exclusive` is entered
    /// reentrantly on `self`.
    #[inline]
    pub(crate) fn with_exclusive<'a, R>(
        &'a self,
        parked: WorkersParkedToken<'a>,
        f: impl FnOnce(&mut PlanPhaseLease<'a>) -> R,
    ) -> R {
        assert_eq!(
            self.instance_id,
            parked.instance_id(),
            "PlanPhaseAuth / WorkersParkedToken MmtkId mismatch"
        );
        assert!(
            !self.leased.swap(true, Ordering::Acquire),
            "Reentrant PlanPhaseAuth::with_exclusive for {:?}",
            self.instance_id
        );

        struct LeaseGuard<'b>(&'b AtomicBool);
        impl Drop for LeaseGuard<'_> {
            #[inline]
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }

        let _guard = LeaseGuard(&self.leased);
        let mut lease = PlanPhaseLease {
            instance_id: self.instance_id,
            _marker: PhantomData,
        };
        let out = f(&mut lease);
        let _ = &parked;
        out
    }
}

/// Exclusive stop-the-world capability lease for a single [`MmtkId`] instance.
///
/// `!Copy`, `!Clone`, `!Send`, `!Sync`, and invariant in `'gc`. Holding `&mut PlanPhaseLease<'gc>`
/// grants exclusive `&mut T` access to any single [`StwCell<T>`] branded with `instance_id` at a
/// time (via Rust's borrow checker on `lease`), or shared `&T` access to arbitrarily many
/// [`StwCell<T>`]s via [`PlanPhaseLease::stw`].
pub(crate) struct PlanPhaseLease<'gc> {
    instance_id: MmtkId,
    _marker: InvariantUnsendUnsync<'gc>,
}

impl PlanPhaseLease<'_> {
    /// Return the [`MmtkId`] instance brand governed by this lease.
    #[inline(always)]
    pub(crate) const fn instance_id(&self) -> MmtkId {
        self.instance_id
    }

    /// Borrow `self` immutably to obtain a shared [`StwToken`] for reading [`StwCell`]s.
    ///
    /// While any `StwToken` (or `&T` borrowed through it) remains live, `self` cannot be mutably
    /// borrowed for [`StwCell::get_mut`].
    #[inline(always)]
    pub(crate) fn stw(&self) -> StwToken<'_> {
        StwToken {
            instance_id: self.instance_id,
            _marker: PhantomData,
        }
    }

    /// Temporarily reborrow the underlying [`WorkersParkedToken`] while `self` is mutably
    /// borrowed, allowing STW phases to call [`super::worker_local::WorkerLocalSlice::iter_mut_all`]
    /// without exiting [`PlanPhaseAuth::with_exclusive`].
    #[inline(always)]
    pub(crate) fn parked(&mut self) -> WorkersParkedToken<'_> {
        WorkersParkedToken {
            instance_id: self.instance_id,
            _marker: PhantomData,
        }
    }
}

/// Shared read-only stop-the-world token borrowed from a [`PlanPhaseLease`].
///
/// `Copy`, `Clone`, `!Send`, `!Sync`, and invariant in `'gc`.
#[derive(Copy, Clone)]
pub(crate) struct StwToken<'gc> {
    instance_id: MmtkId,
    _marker: InvariantUnsendUnsync<'gc>,
}

impl StwToken<'_> {
    /// Return the [`MmtkId`] instance brand governed by this token.
    #[inline(always)]
    pub(crate) const fn instance_id(&self) -> MmtkId {
        self.instance_id
    }
}

/// Instance-branded interior-mutability cell whose contents can be mutated safely during STW
/// phases via [`PlanPhaseLease`] and read via [`StwToken`] without runtime locks on the cell.
pub(crate) struct StwCell<T> {
    instance_id: MmtkId,
    inner: UnsafeCell<T>,
}

// SAFETY: Transferring ownership of `StwCell<T>` across threads moves/drops `T` exclusively, which
// is sound whenever `T: Send`.
unsafe impl<T: Send> Send for StwCell<T> {}

// SAFETY: Accessing `T` through a shared `&StwCell<T>` requires either:
// 1. `&mut PlanPhaseLease<'_>` in `get_mut`, where `PlanPhaseAuth::new` enforces at most one
//    authority per `MmtkId`, `PlanPhaseAuth::with_exclusive` enforces non-reentrancy with
//    `Acquire`/`Release` ordering on `leased` plus a `WorkersParkedToken` witnessing that all
//    workers are parked under the scheduler monitor mutex, so exclusive `&mut T` handoff between
//    sequential STW pauses is synchronized and sound because `T: Send`; or
// 2. `&StwToken<'_>` in `get`, which borrows the unique `PlanPhaseLease` immutably (forbidding any
//    concurrent `get_mut`), and sharing `&T` (e.g., into scoped threads within that STW pause) is
//    sound because `T: Sync`.
unsafe impl<T: Send + Sync> Sync for StwCell<T> {}

impl<T> StwCell<T> {
    /// Create a new `StwCell<T>` branded with `instance_id`.
    #[inline(always)]
    pub(crate) const fn new(instance_id: MmtkId, val: T) -> Self {
        Self {
            instance_id,
            inner: UnsafeCell::new(val),
        }
    }

    /// Return the [`MmtkId`] brand of this cell.
    #[inline(always)]
    pub(crate) const fn instance_id(&self) -> MmtkId {
        self.instance_id
    }

    /// Borrow `&T` immutably using a shared [`StwToken`] with matching `instance_id`.
    ///
    /// # Panics
    ///
    /// Panics if `self.instance_id != token.instance_id`.
    #[inline(always)]
    pub(crate) fn get<'a>(&'a self, token: &'a StwToken<'_>) -> &'a T {
        assert_eq!(
            self.instance_id, token.instance_id,
            "StwCell::get MmtkId mismatch"
        );
        // SAFETY: `token: &'a StwToken<'gc>` can only be obtained from `PlanPhaseLease::stw(&self)`
        // on the unique active `PlanPhaseLease` for `self.instance_id`. Well-formedness of
        // `&'a StwToken<'gc>` requires `'gc: 'a`, keeping `PlanPhaseLease` immutably borrowed for
        // at least `'a` and statically preventing any concurrent `get_mut(&mut lease)` or exit
        // from `PlanPhaseAuth::with_exclusive` while the returned `&'a T` is live.
        unsafe { &*self.inner.get() }
    }

    /// Borrow `&mut T` exclusively using `&mut PlanPhaseLease` with matching `instance_id`.
    ///
    /// Note that if two distinct `StwCell`s need simultaneous `&mut` borrows, wrap both fields in
    /// one phase-state struct inside a single `StwCell`, because `&mut PlanPhaseLease` is uniquely
    /// borrowed by the returned `&'a mut T` for `'a` (preventing same-cell aliasing when two
    /// `&StwCell<T>` references point to the same cell!).
    ///
    /// # Panics
    ///
    /// Panics if `self.instance_id != lease.instance_id`.
    #[inline(always)]
    pub(crate) fn get_mut<'a>(&'a self, lease: &'a mut PlanPhaseLease<'_>) -> &'a mut T {
        assert_eq!(
            self.instance_id, lease.instance_id,
            "StwCell::get_mut MmtkId mismatch"
        );
        // SAFETY: `PlanPhaseAuth::new` and `PlanPhaseAuth::with_exclusive` guarantee that at most
        // one `PlanPhaseLease` exists for `self.instance_id` at any time (with all workers parked
        // and `Acquire`/`Release` synchronization across phases), and borrowing `lease` mutably
        // for `'a` prevents any concurrent `get` (`lease.stw()`) or second `get_mut` (including on
        // an alias `&StwCell<T>` to `self`) for the entire lifetime `'a`.
        unsafe { &mut *self.inner.get() }
    }

    /// Borrow `&mut T` directly when holding exclusive `&mut StwCell<T>` (e.g., during startup or
    /// `Drop`), requiring no token and `0` `unsafe`.
    #[inline(always)]
    pub(crate) fn get_mut_exclusive(&mut self) -> &mut T {
        self.inner.get_mut()
    }

    /// Consume the cell and return the inner value.
    #[inline(always)]
    pub(crate) fn into_inner(self) -> T {
        self.inner.into_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::rust_util::worker_local::WorkerPerm;
    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use std::cell::Cell;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::rc::Rc;

    // Auto-trait & copyability checks on capability tokens and `StwCell`.
    assert_impl_all!(MmtkId: Copy, Clone, Send, Sync);
    assert_not_impl_any!(WorkersParkedToken<'static>: Copy, Clone, Send, Sync);
    assert_not_impl_any!(PlanPhaseLease<'static>: Copy, Clone, Send, Sync);
    assert_impl_all!(StwToken<'static>: Copy, Clone);
    assert_not_impl_any!(StwToken<'static>: Send, Sync);

    assert_impl_all!(StwCell<usize>: Send, Sync);
    assert_impl_all!(StwCell<Cell<usize>>: Send);
    assert_not_impl_any!(StwCell<Cell<usize>>: Sync);
    assert_not_impl_any!(StwCell<Rc<usize>>: Send, Sync);

    #[test]
    fn test_stw_cell_read_write_and_exclusive() {
        let id = MmtkId::fresh();
        let mut perms = WorkerPerm::mint_for_instance(id, 2);
        let auth = PlanPhaseAuth::new(id);
        let mut cell = StwCell::new(id, 10usize);
        let cell_b = StwCell::new(id, 20usize);

        *cell.get_mut_exclusive() = 15;

        let parked = WorkersParkedToken::from_all_perms(id, &mut perms);
        auth.with_exclusive(parked, |lease| {
            {
                let tok = lease.stw();
                let a = cell.get(&tok);
                let b = cell_b.get(&tok);
                assert_eq!(*a + *b, 35);
            }
            *cell.get_mut(lease) += 5;
            let tok = lease.stw();
            assert_eq!(*cell.get(&tok), 20);
        });
    }

    #[test]
    #[should_panic(expected = "StwCell::get_mut MmtkId mismatch")]
    fn test_stw_cell_cross_instance_get_mut_panics() {
        let id1 = MmtkId::fresh();
        let id2 = MmtkId::fresh();
        let mut perms1 = WorkerPerm::mint_for_instance(id1, 1);
        let auth1 = PlanPhaseAuth::new(id1);
        let cell2 = StwCell::new(id2, 99usize);

        let parked1 = WorkersParkedToken::from_all_perms(id1, &mut perms1);
        auth1.with_exclusive(parked1, |lease1| {
            let _ = cell2.get_mut(lease1);
        });
    }

    #[test]
    #[should_panic(expected = "StwCell::get MmtkId mismatch")]
    fn test_stw_cell_cross_instance_get_panics() {
        let id1 = MmtkId::fresh();
        let id2 = MmtkId::fresh();
        let mut perms1 = WorkerPerm::mint_for_instance(id1, 1);
        let auth1 = PlanPhaseAuth::new(id1);
        let cell2 = StwCell::new(id2, 99usize);

        let parked1 = WorkersParkedToken::from_all_perms(id1, &mut perms1);
        auth1.with_exclusive(parked1, |lease1| {
            let tok1 = lease1.stw();
            let _ = cell2.get(&tok1);
        });
    }

    #[test]
    #[should_panic(expected = "PlanPhaseAuth / WorkersParkedToken MmtkId mismatch")]
    fn test_auth_parked_token_mismatch_panics() {
        let id1 = MmtkId::fresh();
        let id2 = MmtkId::fresh();
        let mut perms2 = WorkerPerm::mint_for_instance(id2, 1);
        let auth1 = PlanPhaseAuth::new(id1);

        let parked2 = WorkersParkedToken::from_all_perms(id2, &mut perms2);
        auth1.with_exclusive(parked2, |_lease| {});
    }

    #[test]
    #[should_panic(expected = "PlanPhaseAuth::new called more than once")]
    fn test_duplicate_plan_phase_auth_for_same_instance_panics() {
        let id = MmtkId::fresh();
        let _auth1 = PlanPhaseAuth::new(id);
        let _auth2 = PlanPhaseAuth::new(id);
    }

    #[test]
    fn test_with_exclusive_unwind_releases_lease() {
        let id = MmtkId::fresh();
        let mut perms = WorkerPerm::mint_for_instance(id, 1);
        let auth = PlanPhaseAuth::new(id);
        let cell = StwCell::new(id, 1usize);

        let res = catch_unwind(AssertUnwindSafe(|| {
            let parked = WorkersParkedToken::from_all_perms(id, &mut perms);
            auth.with_exclusive(parked, |lease| {
                *cell.get_mut(lease) = 7;
                panic!("simulated panic in STW callback");
            });
        }));
        assert!(res.is_err());

        // LeaseGuard must have cleared `auth.leased`, allowing subsequent STW phases to proceed.
        let parked = WorkersParkedToken::from_all_perms(id, &mut perms);
        auth.with_exclusive(parked, |lease| {
            let tok = lease.stw();
            assert_eq!(*cell.get(&tok), 7);
        });
    }
}
