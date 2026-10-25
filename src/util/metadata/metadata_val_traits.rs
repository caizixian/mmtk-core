use crate::util::Address;
use atomic_traits::{fetch, Atomic};
use core::sync::atomic::*;
use num_traits::{FromPrimitive, ToPrimitive};
use num_traits::{Unsigned, WrappingAdd, WrappingSub, Zero};

/// Describes bits and log2 bits for the numbers.
/// If num_traits has this, we do not need our own implementation: <https://github.com/rust-num/num-traits/issues/247>
pub trait Bits {
    /// The size of this atomic type in bits.
    const BITS: u32;
    /// The size (in log2) of this atomic type in bits.
    const LOG2: u32;
}
macro_rules! impl_bits_trait {
    ($t: ty) => {
        impl Bits for $t {
            const BITS: u32 = <$t>::BITS;
            const LOG2: u32 = Self::BITS.trailing_zeros();
        }
    };
}
impl_bits_trait!(u8);
impl_bits_trait!(u16);
impl_bits_trait!(u32);
impl_bits_trait!(u64);
impl_bits_trait!(usize);

/// Describes bitwise operations.
/// If num_traits has this, we do not need our own implementation: <https://github.com/rust-num/num-traits/issues/232>
pub trait BitwiseOps {
    /// Perform bitwise and for two values.
    fn bitand(self, other: Self) -> Self;
    /// Perform bitwise or for two values.
    fn bitor(self, other: Self) -> Self;
    /// Perform bitwise xor for two values.
    fn bitxor(self, other: Self) -> Self;
    /// Perform bitwise invert (not) for the value.
    fn inv(self) -> Self;
}
macro_rules! impl_bitwise_ops_trait {
    ($t: ty) => {
        impl BitwiseOps for $t {
            fn bitand(self, other: Self) -> Self {
                self & other
            }
            fn bitor(self, other: Self) -> Self {
                self | other
            }
            fn bitxor(self, other: Self) -> Self {
                self ^ other
            }
            fn inv(self) -> Self {
                !self
            }
        }
    };
}
impl_bitwise_ops_trait!(u8);
impl_bitwise_ops_trait!(u16);
impl_bitwise_ops_trait!(u32);
impl_bitwise_ops_trait!(u64);
impl_bitwise_ops_trait!(usize);

/// Maps a store, RMW, or CAS `success_order` `Ordering` to the corresponding load/fetch
/// `Ordering` (`Relaxed | Release => Relaxed`, `Acquire | AcqRel => Acquire`,
/// `SeqCst => SeqCst`), ensuring emulated CAS/RMW loops never pass `Ordering::Release` or
/// `Ordering::AcqRel` to `Atomic*::load` or `Atomic*::fetch_update`'s `fetch_order`.
#[inline(always)]
pub(crate) const fn load_order_for_rmw(order: Ordering) -> Ordering {
    match order {
        Ordering::Relaxed | Ordering::Release => Ordering::Relaxed,
        Ordering::Acquire | Ordering::AcqRel => Ordering::Acquire,
        Ordering::SeqCst => Ordering::SeqCst,
        _ => Ordering::SeqCst,
    }
}

/// Maximum byte alignment required across non-atomic and atomic accesses of `T` (notably 8 bytes
/// for `u64` / `AtomicU64` even on 32-bit targets where `align_of::<u64>()` may be 4).
#[inline(always)]
pub(crate) const fn max_metadata_align<T: MetadataValue>() -> usize {
    if T::ALIGN >= T::ATOMIC_ALIGN {
        T::ALIGN
    } else {
        T::ATOMIC_ALIGN
    }
}

/// Unconditionally verify (including in `--release`) that `addr` is non-zero and aligned to
/// `T::ATOMIC_ALIGN` (catching misaligned 8-byte `AtomicU64` even on 32-bit architectures).
#[inline(always)]
pub(crate) fn assert_atomic_align<T: MetadataValue>(addr: Address) {
    assert!(
        !addr.is_zero() && addr.is_aligned_to(T::ATOMIC_ALIGN),
        "Unaligned or null atomic metadata access at {} (required align {})",
        addr,
        T::ATOMIC_ALIGN
    );
}

#[doc(hidden)]
pub mod sealed {
    use atomic_traits::{Atomic, Bitwise, NumOps};

    pub trait MetadataAtomic: Copy {
        /// Associated atomic storage cell for `Self` (`AtomicU8`..`AtomicUsize`).
        type Atomic: Atomic<Type = Self> + NumOps + Bitwise + Send + Sync + 'static;

        /// Required byte alignment for non-atomic `Self` accesses.
        const ALIGN: usize = core::mem::align_of::<Self>();

        /// Required byte alignment for atomic `Self` accesses (`align_of::<AtomicU64>() == 8` even
        /// on 32-bit targets where `align_of::<u64>()` may be 4).
        const ATOMIC_ALIGN: usize;
    }
}

/// The number type for accessing metadata.
/// It requires a few traits from num-traits and a few traits we defined above.
/// The methods in this trait are mostly about atomically accessing such types.
///
/// For all `*_atomic`, `compare_exchange`, and `fetch_*` methods, callers must ensure `addr`
/// points to `size_of::<Self::Atomic>()` bytes of mapped, initialized (and writable for stores/RMW)
/// memory accessed solely via `Self::Atomic` without aliasing non-atomic references, and that any
/// `Ordering` arguments are valid for the underlying atomic operation.
pub trait MetadataValue:
    Unsigned
    + Zero
    + WrappingAdd
    + WrappingSub
    + Bits
    + BitwiseOps
    + ToPrimitive
    + Copy
    + FromPrimitive
    + std::fmt::Display
    + std::fmt::Debug
    + sealed::MetadataAtomic
{
    /// Non-atomic load from `addr`.
    ///
    /// # Safety
    /// `addr` must be non-null, aligned to `Self::ALIGN`, and point to mapped, initialized memory
    /// with no racing writes or live `&mut` aliases.
    #[inline(always)]
    unsafe fn load(addr: Address) -> Self {
        assert!(
            !addr.is_zero() && addr.is_aligned_to(Self::ALIGN),
            "Unaligned or null non-atomic metadata load at {} (required align {})",
            addr,
            Self::ALIGN
        );
        // SAFETY: Non-null and `Self::ALIGN` checked above; caller guarantees mapped, unaliased read.
        unsafe { addr.load::<Self>() }
    }

    /// Atomic load from `addr` with memory ordering `order`.
    ///
    /// # Safety
    /// `addr` must satisfy [`MetadataValue`]'s atomic read contract with a valid load `order`.
    #[inline(always)]
    unsafe fn load_atomic(addr: Address, order: Ordering) -> Self {
        assert_atomic_align::<Self>(addr);
        // SAFETY: `assert_atomic_align` checked non-null + `ATOMIC_ALIGN`; caller upholds atomic contract.
        Atomic::load(unsafe { addr.as_ref::<Self::Atomic>() }, order)
    }

    /// Non-atomic store of `value` to `addr`.
    ///
    /// # Safety
    /// `addr` must be non-null, aligned to `Self::ALIGN`, and exclusively point to mapped writable
    /// memory with no concurrent accesses or live `&`/`&mut` aliases.
    #[inline(always)]
    unsafe fn store(addr: Address, value: Self) {
        assert!(
            !addr.is_zero() && addr.is_aligned_to(Self::ALIGN),
            "Unaligned or null non-atomic metadata store at {} (required align {})",
            addr,
            Self::ALIGN
        );
        // SAFETY: Non-null and `Self::ALIGN` checked above; caller guarantees exclusive writable access.
        unsafe { addr.store::<Self>(value) }
    }

    /// Atomic store of `value` to `addr` with memory ordering `order`.
    ///
    /// # Safety
    /// `addr` must satisfy [`MetadataValue`]'s atomic write contract with a valid store `order`.
    #[inline(always)]
    unsafe fn store_atomic(addr: Address, value: Self, order: Ordering) {
        assert_atomic_align::<Self>(addr);
        // SAFETY: `assert_atomic_align` checked non-null + `ATOMIC_ALIGN`; caller upholds atomic contract.
        Atomic::store(unsafe { addr.as_ref::<Self::Atomic>() }, value, order);
    }

    /// Atomic compare-and-exchange at `addr`.
    ///
    /// # Safety
    /// `addr` must satisfy [`MetadataValue`]'s atomic RMW contract with valid CAS orderings.
    #[inline(always)]
    unsafe fn compare_exchange(
        addr: Address,
        current: Self,
        new: Self,
        success: Ordering,
        failure: Ordering,
    ) -> Result<Self, Self> {
        assert_atomic_align::<Self>(addr);
        // SAFETY: `assert_atomic_align` checked non-null + `ATOMIC_ALIGN`; caller upholds atomic contract.
        Atomic::compare_exchange(
            unsafe { addr.as_ref::<Self::Atomic>() },
            current,
            new,
            success,
            failure,
        )
    }

    /// Atomic fetch-and-add at `addr`.
    ///
    /// # Safety
    /// `addr` must satisfy [`MetadataValue`]'s atomic RMW contract.
    #[inline(always)]
    unsafe fn fetch_add(addr: Address, value: Self, order: Ordering) -> Self {
        assert_atomic_align::<Self>(addr);
        // SAFETY: `assert_atomic_align` checked non-null + `ATOMIC_ALIGN`; caller upholds atomic contract.
        fetch::Add::fetch_add(unsafe { addr.as_ref::<Self::Atomic>() }, value, order)
    }

    /// Atomic fetch-and-subtract at `addr`.
    ///
    /// # Safety
    /// `addr` must satisfy [`MetadataValue`]'s atomic RMW contract.
    #[inline(always)]
    unsafe fn fetch_sub(addr: Address, value: Self, order: Ordering) -> Self {
        assert_atomic_align::<Self>(addr);
        // SAFETY: `assert_atomic_align` checked non-null + `ATOMIC_ALIGN`; caller upholds atomic contract.
        fetch::Sub::fetch_sub(unsafe { addr.as_ref::<Self::Atomic>() }, value, order)
    }

    /// Atomic fetch-and-bitwise-and at `addr`.
    ///
    /// # Safety
    /// `addr` must satisfy [`MetadataValue`]'s atomic RMW contract.
    #[inline(always)]
    unsafe fn fetch_and(addr: Address, value: Self, order: Ordering) -> Self {
        assert_atomic_align::<Self>(addr);
        // SAFETY: `assert_atomic_align` checked non-null + `ATOMIC_ALIGN`; caller upholds atomic contract.
        fetch::And::fetch_and(unsafe { addr.as_ref::<Self::Atomic>() }, value, order)
    }

    /// Atomic fetch-and-bitwise-or at `addr`.
    ///
    /// # Safety
    /// `addr` must satisfy [`MetadataValue`]'s atomic RMW contract.
    #[inline(always)]
    unsafe fn fetch_or(addr: Address, value: Self, order: Ordering) -> Self {
        assert_atomic_align::<Self>(addr);
        // SAFETY: `assert_atomic_align` checked non-null + `ATOMIC_ALIGN`; caller upholds atomic contract.
        fetch::Or::fetch_or(unsafe { addr.as_ref::<Self::Atomic>() }, value, order)
    }

    /// Atomic fetch-and-update loop at `addr`.
    ///
    /// # Safety
    /// `addr` must satisfy [`MetadataValue`]'s atomic RMW contract across the entire `fetch_update`
    /// loop (including calls to `f`), with valid `set_order` and `fetch_order`.
    #[inline(always)]
    unsafe fn fetch_update<F>(
        addr: Address,
        set_order: Ordering,
        fetch_order: Ordering,
        f: F,
    ) -> Result<Self, Self>
    where
        F: FnMut(Self) -> Option<Self>,
    {
        assert_atomic_align::<Self>(addr);
        // SAFETY: `assert_atomic_align` checked non-null + `ATOMIC_ALIGN`; caller upholds atomic contract.
        // `atomic-traits 0.4` forwards `(arg0, arg1)` positionally to `Atomic*::fetch_update(set, fetch)`.
        fetch::Update::fetch_update(
            unsafe { addr.as_ref::<Self::Atomic>() },
            set_order,
            fetch_order,
            f,
        )
    }
}

macro_rules! impl_metadata_value_trait {
    ($non_atomic:ty, $atomic:ty) => {
        impl sealed::MetadataAtomic for $non_atomic {
            type Atomic = $atomic;
            const ATOMIC_ALIGN: usize = core::mem::align_of::<$atomic>();
        }
        impl MetadataValue for $non_atomic {}
    };
}
impl_metadata_value_trait!(u8, AtomicU8);
impl_metadata_value_trait!(u16, AtomicU16);
impl_metadata_value_trait!(u32, AtomicU32);
impl_metadata_value_trait!(u64, AtomicU64);
impl_metadata_value_trait!(usize, AtomicUsize);

/// Unified atomic reference to either a full-width metadata cell (`&'a T::Atomic`) or a sub-byte
/// field packed within an [`AtomicU8`] (`shift` in `0..8`, `mask` already shifted to field bits).
///
/// Centralizes all atomic load, store, CAS, and RMW kernels shared by
/// [`super::header_metadata::HeaderMetadataSpec`] and [`super::side_metadata::SideMetadataSpec`].
pub(crate) enum AtomicMetaRef<'a, T: MetadataValue> {
    Full(&'a T::Atomic),
    SubByte {
        byte: &'a AtomicU8,
        shift: u8,
        mask: u8,
    },
}

impl<'a, T: MetadataValue> AtomicMetaRef<'a, T> {
    #[inline(always)]
    fn extract_sub_byte(raw: u8, shift: u8, mask: u8) -> T {
        FromPrimitive::from_u8((raw & mask) >> shift).unwrap()
    }

    #[inline(always)]
    fn splice_sub_byte(raw: u8, val: u8, shift: u8, mask: u8) -> u8 {
        (raw & !mask) | ((val << shift) & mask)
    }

    #[inline(always)]
    pub(crate) fn load(self, optional_mask: Option<T>, order: Ordering) -> T {
        let res = match self {
            Self::SubByte { byte, shift, mask } => {
                Self::extract_sub_byte(byte.load(order), shift, mask)
            }
            Self::Full(atomic) => Atomic::load(atomic, order),
        };
        optional_mask.map_or(res, |mask| res.bitand(mask))
    }

    #[inline(always)]
    pub(crate) fn store(self, val: T, optional_mask: Option<T>, order: Ordering) {
        match self {
            Self::SubByte { byte, shift, mask } => {
                let val_u8 = val.to_u8().unwrap();
                debug_assert!(val_u8 <= (mask >> shift));
                let fetch_order = load_order_for_rmw(order);
                #[allow(deprecated)]
                let _ = byte.fetch_update(order, fetch_order, |raw| {
                    Some(Self::splice_sub_byte(raw, val_u8, shift, mask))
                });
            }
            Self::Full(atomic) => {
                if let Some(mask) = optional_mask {
                    let fetch_order = load_order_for_rmw(order);
                    let _ = fetch::Update::fetch_update(atomic, order, fetch_order, |old| {
                        Some(old.bitand(mask.inv()).bitor(val.bitand(mask)))
                    });
                } else {
                    Atomic::store(atomic, val, order);
                }
            }
        }
    }

    #[inline(always)]
    pub(crate) fn compare_exchange(
        self,
        old_val: T,
        new_val: T,
        optional_mask: Option<T>,
        success_order: Ordering,
        failure_order: Ordering,
    ) -> Result<T, T> {
        match self {
            Self::SubByte { byte, shift, mask } => {
                let old_u8 = old_val.to_u8().unwrap();
                let new_u8 = new_val.to_u8().unwrap();
                debug_assert!(old_u8 <= (mask >> shift) && new_u8 <= (mask >> shift));
                let real_old = byte.load(load_order_for_rmw(success_order));
                let expected_old = Self::splice_sub_byte(real_old, old_u8, shift, mask);
                let expected_new = Self::splice_sub_byte(expected_old, new_u8, shift, mask);
                byte.compare_exchange(expected_old, expected_new, success_order, failure_order)
                    .map(|x| Self::extract_sub_byte(x, shift, mask))
                    .map_err(|x| Self::extract_sub_byte(x, shift, mask))
            }
            Self::Full(atomic) => {
                let (expected_old, expected_new) = if let Some(mask) = optional_mask {
                    let cur = Atomic::load(atomic, load_order_for_rmw(success_order));
                    let preserved = cur.bitand(mask.inv());
                    (
                        preserved.bitor(old_val.bitand(mask)),
                        preserved.bitor(new_val.bitand(mask)),
                    )
                } else {
                    (old_val, new_val)
                };
                Atomic::compare_exchange(
                    atomic,
                    expected_old,
                    expected_new,
                    success_order,
                    failure_order,
                )
            }
        }
    }

    #[inline(always)]
    fn sub_byte_rmw(
        byte: &AtomicU8,
        shift: u8,
        mask: u8,
        order: Ordering,
        f: impl Fn(u8) -> u8,
    ) -> T {
        let fetch_order = load_order_for_rmw(order);
        #[allow(deprecated)]
        let prev_raw = byte
            .fetch_update(order, fetch_order, |raw| {
                Some(Self::splice_sub_byte(
                    raw,
                    f((raw & mask) >> shift),
                    shift,
                    mask,
                ))
            })
            .unwrap();
        Self::extract_sub_byte(prev_raw, shift, mask)
    }

    #[inline(always)]
    pub(crate) fn fetch_add(self, val: T, order: Ordering) -> T {
        match self {
            Self::SubByte { byte, shift, mask } => {
                let rhs = val.to_u8().unwrap();
                Self::sub_byte_rmw(byte, shift, mask, order, |x| x.wrapping_add(rhs))
            }
            Self::Full(atomic) => fetch::Add::fetch_add(atomic, val, order),
        }
    }

    #[inline(always)]
    pub(crate) fn fetch_sub(self, val: T, order: Ordering) -> T {
        match self {
            Self::SubByte { byte, shift, mask } => {
                let rhs = val.to_u8().unwrap();
                Self::sub_byte_rmw(byte, shift, mask, order, |x| x.wrapping_sub(rhs))
            }
            Self::Full(atomic) => fetch::Sub::fetch_sub(atomic, val, order),
        }
    }

    #[inline(always)]
    pub(crate) fn fetch_and(self, val: T, order: Ordering) -> T {
        match self {
            Self::SubByte { byte, shift, mask } => {
                let rhs = (val.to_u8().unwrap() << shift) | !mask;
                Self::extract_sub_byte(byte.fetch_and(rhs, order), shift, mask)
            }
            Self::Full(atomic) => fetch::And::fetch_and(atomic, val, order),
        }
    }

    #[inline(always)]
    pub(crate) fn fetch_or(self, val: T, order: Ordering) -> T {
        match self {
            Self::SubByte { byte, shift, mask } => {
                let rhs = (val.to_u8().unwrap() << shift) & mask;
                Self::extract_sub_byte(byte.fetch_or(rhs, order), shift, mask)
            }
            Self::Full(atomic) => fetch::Or::fetch_or(atomic, val, order),
        }
    }

    #[inline(always)]
    pub(crate) fn fetch_update(
        self,
        set_order: Ordering,
        fetch_order: Ordering,
        mut f: impl FnMut(T) -> Option<T>,
    ) -> Result<T, T> {
        match self {
            Self::SubByte { byte, shift, mask } =>
            {
                #[allow(deprecated)]
                byte.fetch_update(set_order, fetch_order, |raw| {
                    f(Self::extract_sub_byte(raw, shift, mask))
                        .map(|next| Self::splice_sub_byte(raw, next.to_u8().unwrap(), shift, mask))
                })
                .map(|raw| Self::extract_sub_byte(raw, shift, mask))
                .map_err(|raw| Self::extract_sub_byte(raw, shift, mask))
            }
            Self::Full(atomic) => fetch::Update::fetch_update(atomic, set_order, fetch_order, f),
        }
    }
}
