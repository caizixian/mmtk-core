use crate::util::Address;
use crate::util::metadata::side_metadata::helpers::MetadataCursor;
use core::sync::atomic::*;
use num_traits::{FromPrimitive, ToPrimitive};
use num_traits::{Unsigned, WrappingAdd, WrappingSub, Zero};
use atomic_traits::Atomic;

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
    type Atomic: Atomic<Type = Self>;

    /// Non atomic load
    fn load(cursor: MetadataCursor) -> Self;

    /// Atomic load
    fn load_atomic(cursor: MetadataCursor, order: Ordering) -> Self;

    /// Non atomic store
    fn store(cursor: MetadataCursor, value: Self);

    /// Atomic store
    fn store_atomic(cursor: MetadataCursor, value: Self, order: Ordering);

    fn compare_exchange(
        cursor: MetadataCursor,
        current: Self,
        new: Self,
        success: Ordering,
        failure: Ordering,
    ) -> Result<Self, Self>;

    fn fetch_add(cursor: MetadataCursor, value: Self, order: Ordering) -> Self;

    fn fetch_sub(cursor: MetadataCursor, value: Self, order: Ordering) -> Self;

    fn fetch_and(cursor: MetadataCursor, value: Self, order: Ordering) -> Self;

    fn fetch_or(cursor: MetadataCursor, value: Self, order: Ordering) -> Self;

    fn fetch_update<F>(
        cursor: MetadataCursor,
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
            type Atomic = $atomic;

            fn load(cursor: MetadataCursor) -> Self {
                cursor.load()
            }

            fn load_atomic(cursor: MetadataCursor, order: Ordering) -> Self {
                cursor.load_atomic(order)
            }

            fn store(cursor: MetadataCursor, value: Self) {
                cursor.store(value)
            }

            fn store_atomic(cursor: MetadataCursor, value: Self, order: Ordering) {
                cursor.store_atomic(value, order)
            }

            fn compare_exchange(
                cursor: MetadataCursor,
                current: Self,
                new: Self,
                success: Ordering,
                failure: Ordering,
            ) -> Result<Self, Self> {
                cursor.compare_exchange(current, new, success, failure)
            }

            fn fetch_add(cursor: MetadataCursor, value: Self, order: Ordering) -> Self {
                // SAFETY: MetadataCursor is assumed to point to a valid, properly aligned atomic value.
                unsafe { cursor.0.as_ref::<$atomic>().fetch_add(value, order) }
            }

            fn fetch_sub(cursor: MetadataCursor, value: Self, order: Ordering) -> Self {
                // SAFETY: MetadataCursor is assumed to point to a valid, properly aligned atomic value.
                unsafe { cursor.0.as_ref::<$atomic>().fetch_sub(value, order) }
            }

            fn fetch_and(cursor: MetadataCursor, value: Self, order: Ordering) -> Self {
                // SAFETY: MetadataCursor is assumed to point to a valid, properly aligned atomic value.
                unsafe { cursor.0.as_ref::<$atomic>().fetch_and(value, order) }
            }

            fn fetch_or(cursor: MetadataCursor, value: Self, order: Ordering) -> Self {
                // SAFETY: MetadataCursor is assumed to point to a valid, properly aligned atomic value.
                unsafe { cursor.0.as_ref::<$atomic>().fetch_or(value, order) }
            }

            fn fetch_update<F>(
                cursor: MetadataCursor,
                set_order: Ordering,
                fetch_order: Ordering,
                f: F,
            ) -> Result<Self, Self>
            where
                F: FnMut(Self) -> Option<Self>,
            {
                // SAFETY: MetadataCursor is assumed to point to a valid, properly aligned atomic value.
                unsafe {
                    cursor
                        .0
                        .as_ref::<$atomic>()
                        .fetch_update(set_order, fetch_order, f)
                }
            }
        }
    };
}

impl_metadata_value_trait!(u8, AtomicU8);
impl_metadata_value_trait!(u16, AtomicU16);
impl_metadata_value_trait!(u32, AtomicU32);
impl_metadata_value_trait!(u64, AtomicU64);
impl_metadata_value_trait!(usize, AtomicUsize);
