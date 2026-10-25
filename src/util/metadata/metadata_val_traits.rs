#![deny(unsafe_op_in_unsafe_fn)]

use crate::util::Address;
use core::sync::atomic::*;
use num_traits::{FromPrimitive, ToPrimitive};
use num_traits::{Unsigned, WrappingAdd, WrappingSub, Zero};

mod private {
    pub trait Sealed {}
}

/// Safe atomic operations over an integer metadata cell (`AtomicU8`, `AtomicU16`, `AtomicU32`,
/// `AtomicU64`, `AtomicUsize`), paired bijectively with [`MetadataValue::Atomic`].
pub trait AtomicMetadataOps: private::Sealed + Sync + Send + 'static {
    /// Non-atomic integer value type corresponding to this atomic cell.
    type Val: MetadataValue<Atomic = Self>;

    /// Atomically load the value with memory ordering `order`.
    fn load_val(&self, order: Ordering) -> Self::Val;

    /// Atomically store `value` with memory ordering `order`.
    fn store_val(&self, value: Self::Val, order: Ordering);

    /// Atomically compare-and-exchange `current` with `new`.
    fn compare_exchange_val(
        &self,
        current: Self::Val,
        new: Self::Val,
        success: Ordering,
        failure: Ordering,
    ) -> Result<Self::Val, Self::Val>;

    /// Atomically add `value`, returning the previous value.
    fn fetch_add_val(&self, value: Self::Val, order: Ordering) -> Self::Val;

    /// Atomically subtract `value`, returning the previous value.
    fn fetch_sub_val(&self, value: Self::Val, order: Ordering) -> Self::Val;

    /// Atomically bitwise-AND `value`, returning the previous value.
    fn fetch_and_val(&self, value: Self::Val, order: Ordering) -> Self::Val;

    /// Atomically bitwise-OR `value`, returning the previous value.
    fn fetch_or_val(&self, value: Self::Val, order: Ordering) -> Self::Val;

    /// Atomically run a CAS loop applying `f`, returning `Ok(prev)` or `Err(cur)`.
    fn fetch_update_val<F>(
        &self,
        set_order: Ordering,
        fetch_order: Ordering,
        f: F,
    ) -> Result<Self::Val, Self::Val>
    where
        F: FnMut(Self::Val) -> Option<Self::Val>;
}

macro_rules! impl_atomic_metadata_ops {
    ($non_atomic:ty, $atomic:ty) => {
        impl private::Sealed for $atomic {}

        impl AtomicMetadataOps for $atomic {
            type Val = $non_atomic;

            #[inline(always)]
            fn load_val(&self, order: Ordering) -> Self::Val {
                self.load(order)
            }

            #[inline(always)]
            fn store_val(&self, value: Self::Val, order: Ordering) {
                self.store(value, order);
            }

            #[inline(always)]
            fn compare_exchange_val(
                &self,
                current: Self::Val,
                new: Self::Val,
                success: Ordering,
                failure: Ordering,
            ) -> Result<Self::Val, Self::Val> {
                self.compare_exchange(current, new, success, failure)
            }

            #[inline(always)]
            fn fetch_add_val(&self, value: Self::Val, order: Ordering) -> Self::Val {
                self.fetch_add(value, order)
            }

            #[inline(always)]
            fn fetch_sub_val(&self, value: Self::Val, order: Ordering) -> Self::Val {
                self.fetch_sub(value, order)
            }

            #[inline(always)]
            fn fetch_and_val(&self, value: Self::Val, order: Ordering) -> Self::Val {
                self.fetch_and(value, order)
            }

            #[inline(always)]
            fn fetch_or_val(&self, value: Self::Val, order: Ordering) -> Self::Val {
                self.fetch_or(value, order)
            }

            #[inline(always)]
            fn fetch_update_val<F>(
                &self,
                set_order: Ordering,
                fetch_order: Ordering,
                f: F,
            ) -> Result<Self::Val, Self::Val>
            where
                F: FnMut(Self::Val) -> Option<Self::Val>,
            {
                // `try_update()` is not stabilized until 1.95. Migrate to `try_update()` after bumping MSRV.
                #[allow(deprecated)]
                {
                    self.fetch_update(set_order, fetch_order, f)
                }
            }
        }
    };
}
impl_atomic_metadata_ops!(u8, AtomicU8);
impl_atomic_metadata_ops!(u16, AtomicU16);
impl_atomic_metadata_ops!(u32, AtomicU32);
impl_atomic_metadata_ops!(u64, AtomicU64);
impl_atomic_metadata_ops!(usize, AtomicUsize);

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
    + bytemuck::Pod
    + FromPrimitive
    + std::fmt::Display
    + std::fmt::Debug
{
    /// Associated atomic storage cell for `Self` (`AtomicU8`, `AtomicU16`, `AtomicU32`,
    /// `AtomicU64`, or `AtomicUsize`).
    type Atomic: AtomicMetadataOps<Val = Self>;

    /// Required byte alignment for non-atomic `Self` accesses.
    const ALIGN: usize = core::mem::align_of::<Self>();

    /// Required byte alignment for atomic `Self` accesses (`align_of::<AtomicU64>() == 8` even on
    /// 32-bit targets where `align_of::<u64>()` may be 4).
    const ATOMIC_ALIGN: usize = core::mem::align_of::<Self>();

    /// Project a shared atomic reference at `addr` for the duration `'a`.
    ///
    /// Panics unconditionally (including in `--release`) if `addr.is_zero()` or
    /// `!addr.is_aligned_to(Self::ATOMIC_ALIGN)` (catching misaligned 8-byte `AtomicU64` even on
    /// 32-bit architectures).
    ///
    /// # Safety
    /// * `addr` must point to `size_of::<Self::Atomic>()` bytes of mapped, readable/writable,
    ///   initialized memory that remains valid for `'a`.
    /// * No concurrent non-atomic read/write or live non-atomic Rust reference (`&` or `&mut`) may
    ///   alias `[addr, addr + size_of::<Self::Atomic>())` during `'a`; all concurrent accesses must
    ///   use atomic operations of the exact same width (`Self::Atomic`).
    unsafe fn atomic_ref<'a>(addr: Address) -> &'a Self::Atomic;

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
    #[inline(always)]
    unsafe fn load_atomic(addr: Address, order: Ordering) -> Self {
        // SAFETY: Caller upholds `Self::atomic_ref` mapped, initialized, and atomic-only aliasing
        // preconditions for the duration of this load.
        unsafe { Self::atomic_ref(addr) }.load_val(order)
    }

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
    #[inline(always)]
    unsafe fn store_atomic(addr: Address, value: Self, order: Ordering) {
        // SAFETY: Caller upholds `Self::atomic_ref` mapped, initialized, and atomic-only aliasing
        // preconditions for the duration of this store.
        unsafe { Self::atomic_ref(addr) }.store_val(value, order);
    }

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
    #[inline(always)]
    unsafe fn compare_exchange(
        addr: Address,
        current: Self,
        new: Self,
        success: Ordering,
        failure: Ordering,
    ) -> Result<Self, Self> {
        // SAFETY: Caller upholds `Self::atomic_ref` mapped, initialized, and atomic-only aliasing
        // preconditions for the duration of this CAS.
        unsafe { Self::atomic_ref(addr) }.compare_exchange_val(current, new, success, failure)
    }

    /// Atomic fetch-and-add at `addr`.
    ///
    /// # Safety
    /// * `addr` must be non-null, aligned to the required alignment of `Self`'s atomic counterpart,
    ///   and point to `size_of::<Self>()` bytes of mapped, readable and writable, initialized
    ///   memory for the duration of the call.
    /// * No concurrent non-atomic access or live non-atomic Rust reference (`&` or `&mut`) may
    ///   alias `[addr, addr + size_of::<Self>())`; all concurrent accesses must use atomic
    ///   operations of the same width.
    #[inline(always)]
    unsafe fn fetch_add(addr: Address, value: Self, order: Ordering) -> Self {
        // SAFETY: Caller upholds `Self::atomic_ref` mapped, initialized, and atomic-only aliasing
        // preconditions for the duration of this RMW.
        unsafe { Self::atomic_ref(addr) }.fetch_add_val(value, order)
    }

    /// Atomic fetch-and-subtract at `addr`.
    ///
    /// # Safety
    /// * `addr` must be non-null, aligned to the required alignment of `Self`'s atomic counterpart,
    ///   and point to `size_of::<Self>()` bytes of mapped, readable and writable, initialized
    ///   memory for the duration of the call.
    /// * No concurrent non-atomic access or live non-atomic Rust reference (`&` or `&mut`) may
    ///   alias `[addr, addr + size_of::<Self>())`; all concurrent accesses must use atomic
    ///   operations of the same width.
    #[inline(always)]
    unsafe fn fetch_sub(addr: Address, value: Self, order: Ordering) -> Self {
        // SAFETY: Caller upholds `Self::atomic_ref` mapped, initialized, and atomic-only aliasing
        // preconditions for the duration of this RMW.
        unsafe { Self::atomic_ref(addr) }.fetch_sub_val(value, order)
    }

    /// Atomic fetch-and-bitwise-and at `addr`.
    ///
    /// # Safety
    /// * `addr` must be non-null, aligned to the required alignment of `Self`'s atomic counterpart,
    ///   and point to `size_of::<Self>()` bytes of mapped, readable and writable, initialized
    ///   memory for the duration of the call.
    /// * No concurrent non-atomic access or live non-atomic Rust reference (`&` or `&mut`) may
    ///   alias `[addr, addr + size_of::<Self>())`; all concurrent accesses must use atomic
    ///   operations of the same width.
    #[inline(always)]
    unsafe fn fetch_and(addr: Address, value: Self, order: Ordering) -> Self {
        // SAFETY: Caller upholds `Self::atomic_ref` mapped, initialized, and atomic-only aliasing
        // preconditions for the duration of this RMW.
        unsafe { Self::atomic_ref(addr) }.fetch_and_val(value, order)
    }

    /// Atomic fetch-and-bitwise-or at `addr`.
    ///
    /// # Safety
    /// * `addr` must be non-null, aligned to the required alignment of `Self`'s atomic counterpart,
    ///   and point to `size_of::<Self>()` bytes of mapped, readable and writable, initialized
    ///   memory for the duration of the call.
    /// * No concurrent non-atomic access or live non-atomic Rust reference (`&` or `&mut`) may
    ///   alias `[addr, addr + size_of::<Self>())`; all concurrent accesses must use atomic
    ///   operations of the same width.
    #[inline(always)]
    unsafe fn fetch_or(addr: Address, value: Self, order: Ordering) -> Self {
        // SAFETY: Caller upholds `Self::atomic_ref` mapped, initialized, and atomic-only aliasing
        // preconditions for the duration of this RMW.
        unsafe { Self::atomic_ref(addr) }.fetch_or_val(value, order)
    }

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
        // SAFETY: Caller upholds `Self::atomic_ref` mapped, initialized, and atomic-only aliasing
        // preconditions across the entire `fetch_update` loop.
        unsafe { Self::atomic_ref(addr) }.fetch_update_val(set_order, fetch_order, f)
    }
}

macro_rules! impl_metadata_value_trait {
    ($non_atomic: ty, $atomic: ty) => {
        impl MetadataValue for $non_atomic {
            type Atomic = $atomic;
            const ALIGN: usize = core::mem::align_of::<$non_atomic>();
            const ATOMIC_ALIGN: usize = core::mem::align_of::<$atomic>();

            #[inline(always)]
            unsafe fn atomic_ref<'a>(addr: Address) -> &'a Self::Atomic {
                assert!(
                    !addr.is_zero() && addr.is_aligned_to(Self::ATOMIC_ALIGN),
                    "Unaligned or null atomic metadata access at {} (required align {})",
                    addr,
                    Self::ATOMIC_ALIGN
                );
                // SAFETY: Non-null and `Self::ATOMIC_ALIGN` alignment were unconditionally verified
                // above; caller guarantees `[addr, addr + size_of::<$atomic>())` is mapped,
                // initialized, valid for `'a`, and accessed solely via `$atomic` without aliasing
                // non-atomic Rust references.
                unsafe { addr.as_ref::<$atomic>() }
            }

            #[inline(always)]
            unsafe fn load(addr: Address) -> Self {
                assert!(
                    !addr.is_zero() && addr.is_aligned_to(Self::ALIGN),
                    "Unaligned or null non-atomic metadata load at {} (required align {})",
                    addr,
                    Self::ALIGN
                );
                // SAFETY: Non-null and `Self::ALIGN` alignment were unconditionally verified above;
                // caller guarantees `addr` points to mapped, initialized memory without racing
                // writes or `&mut` aliases.
                unsafe { addr.load::<$non_atomic>() }
            }

            #[inline(always)]
            unsafe fn store(addr: Address, value: Self) {
                assert!(
                    !addr.is_zero() && addr.is_aligned_to(Self::ALIGN),
                    "Unaligned or null non-atomic metadata store at {} (required align {})",
                    addr,
                    Self::ALIGN
                );
                // SAFETY: Non-null and `Self::ALIGN` alignment were unconditionally verified above;
                // caller guarantees `addr` points to mapped writable memory with exclusive access
                // and no aliasing Rust references.
                unsafe { addr.store::<$non_atomic>(value) }
            }
        }
    };
}
impl_metadata_value_trait!(u8, AtomicU8);
impl_metadata_value_trait!(u16, AtomicU16);
impl_metadata_value_trait!(u32, AtomicU32);
impl_metadata_value_trait!(u64, AtomicU64);
impl_metadata_value_trait!(usize, AtomicUsize);
