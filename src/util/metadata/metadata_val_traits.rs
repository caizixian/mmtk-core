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
    /// The associated atomic type.
    type Atomic;

    /// Non atomic load
    fn load(non_atomic: &Self) -> Self {
        *non_atomic
    }

    /// Atomic load
    fn load_atomic(atomic: &Self::Atomic, order: Ordering) -> Self;

    /// Non atomic store
    fn store(non_atomic: &mut Self, value: Self) {
        *non_atomic = value;
    }

    /// Atomic store
    fn store_atomic(atomic: &Self::Atomic, value: Self, order: Ordering);

    fn compare_exchange(
        atomic: &Self::Atomic,
        current: Self,
        new: Self,
        success: Ordering,
        failure: Ordering,
    ) -> Result<Self, Self>;

    fn fetch_add(atomic: &Self::Atomic, value: Self, order: Ordering) -> Self;

    fn fetch_sub(atomic: &Self::Atomic, value: Self, order: Ordering) -> Self;

    fn fetch_and(atomic: &Self::Atomic, value: Self, order: Ordering) -> Self;

    fn fetch_or(atomic: &Self::Atomic, value: Self, order: Ordering) -> Self;

    fn fetch_update<F>(
        atomic: &Self::Atomic,
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

            fn load_atomic(atomic: &Self::Atomic, order: Ordering) -> Self {
                atomic.load(order)
            }

            fn store_atomic(atomic: &Self::Atomic, value: Self, order: Ordering) {
                atomic.store(value, order)
            }

            fn compare_exchange(
                atomic: &Self::Atomic,
                current: Self,
                new: Self,
                success: Ordering,
                failure: Ordering,
            ) -> Result<Self, Self> {
                atomic.compare_exchange(current, new, success, failure)
            }

            fn fetch_add(atomic: &Self::Atomic, value: Self, order: Ordering) -> Self {
                atomic.fetch_add(value, order)
            }

            fn fetch_sub(atomic: &Self::Atomic, value: Self, order: Ordering) -> Self {
                atomic.fetch_sub(value, order)
            }

            fn fetch_and(atomic: &Self::Atomic, value: Self, order: Ordering) -> Self {
                atomic.fetch_and(value, order)
            }

            fn fetch_or(atomic: &Self::Atomic, value: Self, order: Ordering) -> Self {
                atomic.fetch_or(value, order)
            }

            fn fetch_update<F>(
                atomic: &Self::Atomic,
                set_order: Ordering,
                fetch_order: Ordering,
                f: F,
            ) -> Result<Self, Self>
            where
                F: FnMut(Self) -> Option<Self>,
            {
                atomic.fetch_update(set_order, fetch_order, f)
            }
        }
    };
}

impl_metadata_value_trait!(u8, AtomicU8);
impl_metadata_value_trait!(u16, AtomicU16);
impl_metadata_value_trait!(u32, AtomicU32);
impl_metadata_value_trait!(u64, AtomicU64);
impl_metadata_value_trait!(usize, AtomicUsize);
