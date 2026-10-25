use crate::util::Address;
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

/// The number type for accessing metadata.
/// It requires a few traits from num-traits and a few traits we defined above.
/// The methods in this trait are mostly about atomically accessing such types.
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
{
    /// Required byte alignment for non-atomic `Self` accesses.
    const ALIGN: usize = core::mem::align_of::<Self>();

    /// Required byte alignment for atomic `Self` accesses (`align_of::<AtomicU64>() == 8` even on
    /// 32-bit targets where `align_of::<u64>()` may be 4).
    const ATOMIC_ALIGN: usize = core::mem::align_of::<Self>();

    /// Non-atomic load from `addr`.
    ///
    /// # Safety
    /// * `addr` must be non-null, aligned to `align_of::<Self>()`, and point to `size_of::<Self>()`
    ///   bytes of mapped, readable, initialized memory.
    /// * No other thread may concurrently write to or atomically modify the memory at `addr`
    ///   (non-atomic reads racing with writes or atomic modifications are undefined behavior), and
    ///   no live Rust `&mut` reference may alias `[addr, addr + size_of::<Self>())`.
    unsafe fn load(addr: Address) -> Self;

    /// Atomic load from `addr` with memory ordering `order`.
    ///
    /// # Safety
    /// * `addr` must be non-null, aligned to the required alignment of `Self`'s atomic counterpart
    ///   (e.g. `align_of::<AtomicU64>()`), and point to `size_of::<Self>()` bytes of mapped,
    ///   readable, initialized memory for the duration of the call.
    /// * `order` must be a valid load ordering (`Relaxed`, `Acquire`, or `SeqCst`).
    /// * No concurrent non-atomic write or live Rust `&mut` reference may alias
    ///   `[addr, addr + size_of::<Self>())`; all concurrent accesses must use atomic operations of
    ///   the same width.
    unsafe fn load_atomic(addr: Address, order: Ordering) -> Self;

    /// Non-atomic store of `value` to `addr`.
    ///
    /// # Safety
    /// * `addr` must be non-null, aligned to `align_of::<Self>()`, and point to `size_of::<Self>()`
    ///   bytes of mapped, writable memory.
    /// * The caller must have exclusive access to `[addr, addr + size_of::<Self>())`: no other
    ///   thread may concurrently read, write, or atomically access this memory, and no live Rust
    ///   reference (`&` or `&mut`) may alias it.
    unsafe fn store(addr: Address, value: Self);

    /// Atomic store of `value` to `addr` with memory ordering `order`.
    ///
    /// # Safety
    /// * `addr` must be non-null, aligned to the required alignment of `Self`'s atomic counterpart,
    ///   and point to `size_of::<Self>()` bytes of mapped, writable, initialized memory for the
    ///   duration of the call.
    /// * `order` must be a valid store ordering (`Relaxed`, `Release`, or `SeqCst`).
    /// * No concurrent non-atomic read/write or live non-atomic Rust reference (`&` or `&mut`) may
    ///   alias `[addr, addr + size_of::<Self>())`; all concurrent accesses must use atomic
    ///   operations of the same width.
    unsafe fn store_atomic(addr: Address, value: Self, order: Ordering);

    /// Atomic compare-and-exchange at `addr`.
    ///
    /// # Safety
    /// * `addr` must be non-null, aligned to the required alignment of `Self`'s atomic counterpart,
    ///   and point to `size_of::<Self>()` bytes of mapped, readable and writable, initialized
    ///   memory for the duration of the call.
    /// * `success` and `failure` must be valid `compare_exchange` orderings (`failure` may not be
    ///   `Release` or `AcqRel`).
    /// * No concurrent non-atomic access or live non-atomic Rust reference (`&` or `&mut`) may
    ///   alias `[addr, addr + size_of::<Self>())`; all concurrent accesses must use atomic
    ///   operations of the same width.
    unsafe fn compare_exchange(
        addr: Address,
        current: Self,
        new: Self,
        success: Ordering,
        failure: Ordering,
    ) -> Result<Self, Self>;

    /// Atomic fetch-and-add at `addr`.
    ///
    /// # Safety
    /// * `addr` must be non-null, aligned to the required alignment of `Self`'s atomic counterpart,
    ///   and point to `size_of::<Self>()` bytes of mapped, readable and writable, initialized
    ///   memory for the duration of the call.
    /// * No concurrent non-atomic access or live non-atomic Rust reference (`&` or `&mut`) may
    ///   alias `[addr, addr + size_of::<Self>())`; all concurrent accesses must use atomic
    ///   operations of the same width.
    unsafe fn fetch_add(addr: Address, value: Self, order: Ordering) -> Self;

    /// Atomic fetch-and-subtract at `addr`.
    ///
    /// # Safety
    /// * `addr` must be non-null, aligned to the required alignment of `Self`'s atomic counterpart,
    ///   and point to `size_of::<Self>()` bytes of mapped, readable and writable, initialized
    ///   memory for the duration of the call.
    /// * No concurrent non-atomic access or live non-atomic Rust reference (`&` or `&mut`) may
    ///   alias `[addr, addr + size_of::<Self>())`; all concurrent accesses must use atomic
    ///   operations of the same width.
    unsafe fn fetch_sub(addr: Address, value: Self, order: Ordering) -> Self;

    /// Atomic fetch-and-bitwise-and at `addr`.
    ///
    /// # Safety
    /// * `addr` must be non-null, aligned to the required alignment of `Self`'s atomic counterpart,
    ///   and point to `size_of::<Self>()` bytes of mapped, readable and writable, initialized
    ///   memory for the duration of the call.
    /// * No concurrent non-atomic access or live non-atomic Rust reference (`&` or `&mut`) may
    ///   alias `[addr, addr + size_of::<Self>())`; all concurrent accesses must use atomic
    ///   operations of the same width.
    unsafe fn fetch_and(addr: Address, value: Self, order: Ordering) -> Self;

    /// Atomic fetch-and-bitwise-or at `addr`.
    ///
    /// # Safety
    /// * `addr` must be non-null, aligned to the required alignment of `Self`'s atomic counterpart,
    ///   and point to `size_of::<Self>()` bytes of mapped, readable and writable, initialized
    ///   memory for the duration of the call.
    /// * No concurrent non-atomic access or live non-atomic Rust reference (`&` or `&mut`) may
    ///   alias `[addr, addr + size_of::<Self>())`; all concurrent accesses must use atomic
    ///   operations of the same width.
    unsafe fn fetch_or(addr: Address, value: Self, order: Ordering) -> Self;

    /// Atomic fetch-and-update loop at `addr`.
    ///
    /// # Safety
    /// * `addr` must be non-null, aligned to the required alignment of `Self`'s atomic counterpart,
    ///   and point to `size_of::<Self>()` bytes of mapped, readable and writable, initialized
    ///   memory for the duration of the call (including across invocations of `f`).
    /// * `set_order` and `fetch_order` must be valid `fetch_update` orderings (`fetch_order` may
    ///   not be `Release` or `AcqRel`).
    /// * No concurrent non-atomic access or live non-atomic Rust reference (`&` or `&mut`) may
    ///   alias `[addr, addr + size_of::<Self>())` (including inside `f`); all concurrent accesses
    ///   must use atomic operations of the same width.
    unsafe fn fetch_update<F>(
        addr: Address,
        set_order: Ordering,
        fetch_order: Ordering,
        f: F,
    ) -> Result<Self, Self>
    where
        F: FnMut(Self) -> Option<Self>;
}

macro_rules! impl_metadata_value_trait {
    ($non_atomic: ty, $atomic: ty) => {
        impl MetadataValue for $non_atomic {
            const ALIGN: usize = core::mem::align_of::<$non_atomic>();
            const ATOMIC_ALIGN: usize = core::mem::align_of::<$atomic>();

            unsafe fn load(addr: Address) -> Self {
                debug_assert!(!addr.is_zero());
                debug_assert!(addr.is_aligned_to(core::mem::align_of::<$non_atomic>()));
                // SAFETY: Caller guarantees `addr` is non-null, aligned to `$non_atomic`, points to
                // mapped and initialized memory, and has no racing writes or `&mut` aliases.
                unsafe { addr.load::<$non_atomic>() }
            }

            unsafe fn load_atomic(addr: Address, order: Ordering) -> Self {
                debug_assert!(!addr.is_zero());
                debug_assert!(addr.is_aligned_to(core::mem::align_of::<$atomic>()));
                // SAFETY: Caller guarantees `addr` is non-null, aligned to `$atomic`, points to
                // mapped and initialized memory without conflicting non-atomic accesses or `&mut`
                // aliases for the duration of this call; `$atomic` has identical size and validity
                // invariants to `$non_atomic`.
                unsafe { addr.as_ref::<$atomic>() }.load(order)
            }

            unsafe fn store(addr: Address, value: Self) {
                debug_assert!(!addr.is_zero());
                debug_assert!(addr.is_aligned_to(core::mem::align_of::<$non_atomic>()));
                // SAFETY: Caller guarantees `addr` is non-null, aligned to `$non_atomic`, points to
                // mapped writable memory, and is exclusively accessed without concurrent readers,
                // writers, or aliasing Rust references.
                unsafe { addr.store::<$non_atomic>(value) }
            }

            unsafe fn store_atomic(addr: Address, value: Self, order: Ordering) {
                debug_assert!(!addr.is_zero());
                debug_assert!(addr.is_aligned_to(core::mem::align_of::<$atomic>()));
                // SAFETY: Caller guarantees `addr` is non-null, aligned to `$atomic`, and points to
                // mapped, writable, initialized memory with no conflicting non-atomic accesses or
                // non-atomic Rust references for the duration of this call.
                unsafe { addr.as_ref::<$atomic>() }.store(value, order)
            }

            unsafe fn compare_exchange(
                addr: Address,
                current: Self,
                new: Self,
                success: Ordering,
                failure: Ordering,
            ) -> Result<Self, Self> {
                debug_assert!(!addr.is_zero());
                debug_assert!(addr.is_aligned_to(core::mem::align_of::<$atomic>()));
                // SAFETY: Caller guarantees `addr` is non-null, aligned to `$atomic`, and points to
                // mapped, readable/writable, initialized memory with no conflicting non-atomic
                // accesses or non-atomic Rust references for the duration of this call.
                unsafe { addr.as_ref::<$atomic>() }.compare_exchange(current, new, success, failure)
            }

            unsafe fn fetch_add(addr: Address, value: Self, order: Ordering) -> Self {
                debug_assert!(!addr.is_zero());
                debug_assert!(addr.is_aligned_to(core::mem::align_of::<$atomic>()));
                // SAFETY: Caller guarantees `addr` is non-null, aligned to `$atomic`, and points to
                // mapped, readable/writable, initialized memory with no conflicting non-atomic
                // accesses or non-atomic Rust references for the duration of this call.
                unsafe { addr.as_ref::<$atomic>() }.fetch_add(value, order)
            }

            unsafe fn fetch_sub(addr: Address, value: Self, order: Ordering) -> Self {
                debug_assert!(!addr.is_zero());
                debug_assert!(addr.is_aligned_to(core::mem::align_of::<$atomic>()));
                // SAFETY: Caller guarantees `addr` is non-null, aligned to `$atomic`, and points to
                // mapped, readable/writable, initialized memory with no conflicting non-atomic
                // accesses or non-atomic Rust references for the duration of this call.
                unsafe { addr.as_ref::<$atomic>() }.fetch_sub(value, order)
            }

            unsafe fn fetch_and(addr: Address, value: Self, order: Ordering) -> Self {
                debug_assert!(!addr.is_zero());
                debug_assert!(addr.is_aligned_to(core::mem::align_of::<$atomic>()));
                // SAFETY: Caller guarantees `addr` is non-null, aligned to `$atomic`, and points to
                // mapped, readable/writable, initialized memory with no conflicting non-atomic
                // accesses or non-atomic Rust references for the duration of this call.
                unsafe { addr.as_ref::<$atomic>() }.fetch_and(value, order)
            }

            unsafe fn fetch_or(addr: Address, value: Self, order: Ordering) -> Self {
                debug_assert!(!addr.is_zero());
                debug_assert!(addr.is_aligned_to(core::mem::align_of::<$atomic>()));
                // SAFETY: Caller guarantees `addr` is non-null, aligned to `$atomic`, and points to
                // mapped, readable/writable, initialized memory with no conflicting non-atomic
                // accesses or non-atomic Rust references for the duration of this call.
                unsafe { addr.as_ref::<$atomic>() }.fetch_or(value, order)
            }

            unsafe fn fetch_update<F>(
                addr: Address,
                set_order: Ordering,
                fetch_order: Ordering,
                f: F,
            ) -> Result<Self, Self>
            where
                F: FnMut(Self) -> Option<Self>,
            {
                debug_assert!(!addr.is_zero());
                debug_assert!(addr.is_aligned_to(core::mem::align_of::<$atomic>()));
                // `try_update()` is not stablized until 1.95.  Migrate to `try_update()` after bumping MSRV.
                #[allow(deprecated)]
                // SAFETY: Caller guarantees `addr` is non-null, aligned to `$atomic`, and points to
                // mapped, readable/writable, initialized memory with no conflicting non-atomic
                // accesses or non-atomic Rust references across the entire `fetch_update` loop.
                unsafe { addr.as_ref::<$atomic>() }.fetch_update(set_order, fetch_order, f)
            }
        }
    };
}
impl_metadata_value_trait!(u8, AtomicU8);
impl_metadata_value_trait!(u16, AtomicU16);
impl_metadata_value_trait!(u32, AtomicU32);
impl_metadata_value_trait!(u64, AtomicU64);
impl_metadata_value_trait!(usize, AtomicUsize);
