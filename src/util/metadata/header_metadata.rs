//! This module provides a default implementation of the access functions for in-header metadata.

use atomic::Ordering;
use std::fmt;

use crate::util::constants::{BITS_IN_BYTE, LOG_BITS_IN_BYTE};
use crate::util::metadata::metadata_val_traits::*;
use crate::util::Address;
use num_traits::FromPrimitive;

/// This struct stores the specification of a header metadata bit-set.
/// It supports either bits metadata of 1-7 bits in the same byte, or u8/u16/u32/u64 at an offset of their natural alignment.
///
/// For performance reasons, objects of this struct should be constants.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct HeaderMetadataSpec {
    /// `bit_offset` is the index of the starting bit from which the data should be read or written.
    /// It is counted from the right (least significant bit) of the byte.
    /// Positive values refer to the bit positions within the current byte, starting with 0 for the
    /// least significant bit (rightmost) up to 7 for the most significant bit (leftmost).
    /// Negative values are used to refer to bit positions in the previous bytes, where -1 indicates
    /// the most significant bit (leftmost) of the byte immediately before the current one.
    pub bit_offset: isize,
    /// `num_of_bits` specifies the number of consecutive bits to be read or written starting from the `bit_offset`.
    /// This value is used to define the size of the data field in bits. For instance, if `num_of_bits` is set to 1,
    /// only a single bit is considered, whereas a value of 8 would indicate a full byte.
    /// This field must be a positive integer and typically should not exceed the size of the data type that
    /// will hold the extracted value (for example, 8 bits for a `u8`, 16 bits for a `u16`, etc.).
    /// The `num_of_bits` together with the `bit_offset` enables the extraction of bit fields of arbitrary
    /// length and position, facilitating bit-level data manipulation.
    pub num_of_bits: usize,
}

impl HeaderMetadataSpec {
    /// We only allow mask for u8/u16/u32/u64/usize. If a mask is used with a spec that does not allow it, this method will panic.
    ///
    /// We allow using mask for certain operations. The reason for mask is that for header metadata, we may have overlapping metadata specs. For example,
    /// a forwarding pointer is pointer-size, but its last 2 bits could be used as forwarding bits. In that case, all accesses to the forwarding pointer
    /// spec should be used with a mask to make sure that we exclude the forwarding bits.
    #[inline(always)]
    pub(crate) const fn assert_mask<T: MetadataValue>(&self, mask: Option<T>) {
        assert!(
            mask.is_none() || self.num_of_bits >= 8,
            "optional_mask is only supported for 8X-bits in-header metadata."
        );
    }

    /// Assert that `self` has valid bit-width and natural/atomic alignment for `T`.
    /// Because real `HeaderMetadataSpec` values are `const`, LLVM constant-folds these checks to
    /// zero instructions in release builds while preventing UB on arbitrary `pub`-field specs.
    #[inline(always)]
    pub(crate) const fn assert_spec<T: MetadataValue>(&self) {
        if self.num_of_bits == 0 {
            panic!("Metadata of 0 bits is not allowed.");
        } else if self.num_of_bits < 8 {
            let Some(last_bit) = self.bit_offset.checked_add(self.num_of_bits as isize - 1) else {
                panic!("Metadata << 8-bits: bit_offset + num_of_bits overflows isize!");
            };
            assert!(
                (self.bit_offset >> LOG_BITS_IN_BYTE) == (last_bit >> LOG_BITS_IN_BYTE),
                "Metadata << 8-bits stretches over two bytes!"
            );
        } else if self.num_of_bits <= 64 {
            assert!(
                self.num_of_bits == T::BITS as usize,
                "HeaderMetadataSpec num_of_bits must equal T::BITS for >= 8-bit metadata"
            );
            let req_align_bits = (max_metadata_align::<T>() * BITS_IN_BYTE) as isize;
            assert!(
                self.bit_offset.rem_euclid(req_align_bits) == 0,
                "HeaderMetadataSpec bit_offset must be aligned to max_metadata_align::<T>()"
            );
        } else {
            // num_of_bits larger than 64
            panic!("Metadata that is larger than 64-bits is not supported");
        }
    }

    const fn byte_offset(&self) -> isize {
        self.bit_offset >> LOG_BITS_IN_BYTE
    }

    fn meta_addr(&self, header: Address) -> Address {
        header + self.byte_offset()
    }

    /// Validate `self` for `T` and project the underlying atomic metadata cell at `header`.
    ///
    /// # Safety
    /// * `self.meta_addr(header)` must point to mapped, initialized, readable/writable
    ///   object-header memory valid for `'a`, with no concurrent non-atomic accesses or live
    ///   non-atomic Rust references (`&` or `&mut`), and all concurrent accesses must use atomic
    ///   operations of the same width (`AtomicU8` when `self.num_of_bits < 8`, or `T::Atomic`).
    #[inline(always)]
    unsafe fn resolve_atomic<'a, T: MetadataValue>(&self, header: Address) -> AtomicMetaRef<'a, T> {
        self.assert_spec::<T>();
        let addr = self.meta_addr(header);
        // SAFETY: `assert_spec::<T>()` verified sub-byte containment (`num_of_bits < 8`) or
        // `num_of_bits == T::BITS` with `bit_offset` alignment, `assert_atomic_align`
        // unconditionally checks non-null and `ATOMIC_ALIGN`, and caller guarantees `addr` is
        // mapped, initialized, and accessed solely via the corresponding atomic width for `'a`.
        unsafe {
            if self.num_of_bits < 8 {
                assert_atomic_align::<u8>(addr);
                let (shift, mask) = self.get_shift_and_mask_for_bits();
                AtomicMetaRef::SubByte {
                    byte: addr.as_ref::<core::sync::atomic::AtomicU8>(),
                    shift: shift as u8,
                    mask,
                }
            } else {
                assert_atomic_align::<T>(addr);
                AtomicMetaRef::Full(addr.as_ref::<T::Atomic>())
            }
        }
    }

    // Some common methods for header metadata that is smaller than 1 byte.

    /// Get the bit shift (the bit distance from the lowest bit to the bits location defined in the spec),
    /// and the mask (used to extract value for the bits defined in the spec).
    fn get_shift_and_mask_for_bits(&self) -> (isize, u8) {
        debug_assert!(self.num_of_bits < BITS_IN_BYTE);
        let byte_offset = self.byte_offset();
        let bit_shift = self.bit_offset - (byte_offset << LOG_BITS_IN_BYTE);
        let mask = ((1u8 << self.num_of_bits) - 1) << bit_shift;
        (bit_shift, mask)
    }

    /// Extract bits from a raw byte, and put it to the lowest bits.
    fn get_bits_from_u8(&self, raw_byte: u8) -> u8 {
        debug_assert!(self.num_of_bits < BITS_IN_BYTE);
        let (bit_shift, mask) = self.get_shift_and_mask_for_bits();
        (raw_byte & mask) >> bit_shift
    }

    /// Set bits to a raw byte. `set_val` has the valid value in its lowest bits.
    fn set_bits_to_u8(&self, raw_byte: u8, set_val: u8) -> u8 {
        debug_assert!(self.num_of_bits < BITS_IN_BYTE);
        debug_assert!(
            set_val < (1 << self.num_of_bits),
            "{:b} exceeds the maximum value of {} bits in the spec",
            set_val,
            self.num_of_bits
        );
        let (bit_shift, mask) = self.get_shift_and_mask_for_bits();
        (raw_byte & !mask) | (set_val << bit_shift)
    }

    /// This function provides a default implementation for the `load_metadata` method from the `ObjectModel` trait.
    ///
    /// # Safety
    /// * `header` must be the valid in-object header base address of a live object such that
    ///   `header + (self.bit_offset >> 3)` is non-null, aligned to `u8` (if `self.num_of_bits < 8`)
    ///   or `T` (if `self.num_of_bits >= 8`), and points to mapped, initialized object-header memory.
    /// * `self` must be a valid `HeaderMetadataSpec` for `T` (`optional_mask.is_none()` when
    ///   `self.num_of_bits < 8`).
    /// * This is a non-atomic load: no other thread may concurrently write to or atomically modify
    ///   the underlying header byte/word, and no live `&mut` reference may alias it.
    pub unsafe fn load<T: MetadataValue>(&self, header: Address, optional_mask: Option<T>) -> T {
        self.assert_mask::<T>(optional_mask);
        self.assert_spec::<T>();

        let addr = self.meta_addr(header);
        // SAFETY: `assert_spec` verified sub-byte containment (`num_of_bits < 8`, align 1) or
        // `num_of_bits == T::BITS` with `bit_offset` alignment, `MetadataValue::load` checks
        // non-null and `T::ALIGN`, and caller guarantees `addr` points to mapped, initialized
        // header memory without racing writes or `&mut` aliases.
        let res: T = unsafe {
            if self.num_of_bits < 8 {
                let byte_val = <u8 as MetadataValue>::load(addr);
                FromPrimitive::from_u8(self.get_bits_from_u8(byte_val)).unwrap()
            } else {
                T::load(addr)
            }
        };

        if let Some(mask) = optional_mask {
            res.bitand(mask)
        } else {
            res
        }
    }

    /// This function provides a default implementation for the `load_metadata_atomic` method from the `ObjectModel` trait.
    pub fn load_atomic<T: MetadataValue>(
        &self,
        header: Address,
        optional_mask: Option<T>,
        ordering: Ordering,
    ) -> T {
        self.assert_mask::<T>(optional_mask);
        // SAFETY: `ObjectModel` contract guarantees `self.meta_addr(header)` is a mapped,
        // initialized header cell accessed atomically without aliasing non-atomic references.
        unsafe { self.resolve_atomic::<T>(header) }.load(optional_mask, ordering)
    }

    /// This function provides a default implementation for the `store_metadata` method from the `ObjectModel` trait.
    ///
    /// # Safety
    /// * `header` must be the valid in-object header base address of a live object such that
    ///   `header + (self.bit_offset >> 3)` is non-null, aligned to `u8` (if `self.num_of_bits < 8`)
    ///   or `T` (if `self.num_of_bits >= 8`), and points to mapped, writable (and initialized when
    ///   `self.num_of_bits < 8` or `optional_mask.is_some()`) object-header memory.
    /// * `self` must be a valid `HeaderMetadataSpec` for `T`, and `val` must fit within
    ///   `self.num_of_bits`.
    /// * This is a non-atomic store (and a non-atomic read-modify-write for sub-byte or masked
    ///   specs): the caller must have exclusive access to the underlying header byte/word with no
    ///   concurrent reads, writes, atomic operations, or live Rust references (`&` or `&mut`).
    pub unsafe fn store<T: MetadataValue>(
        &self,
        header: Address,
        val: T,
        optional_mask: Option<T>,
    ) {
        self.assert_mask::<T>(optional_mask);
        self.assert_spec::<T>();

        let addr = self.meta_addr(header);
        // SAFETY: `assert_spec` verified bit-width and `T` alignment; `MetadataValue::{load,store}`
        // check non-null and `T::ALIGN`; caller guarantees mapped, exclusively writable memory.
        unsafe {
            if self.num_of_bits < 8 {
                let old_byte = <u8 as MetadataValue>::load(addr);
                let new_byte = self.set_bits_to_u8(old_byte, val.to_u8().unwrap());
                <u8 as MetadataValue>::store(addr, new_byte);
            } else if let Some(mask) = optional_mask {
                let old_val = T::load(addr);
                T::store(addr, old_val.bitand(mask.inv()).bitor(val.bitand(mask)));
            } else {
                T::store(addr, val);
            }
        }
    }

    /// This function provides a default implementation for the `store_metadata_atomic` method from the `ObjectModel` trait.
    ///
    /// Note: for sub-byte or masked metadata, this function performs a compare-and-swap loop so the update always succeeds.
    pub fn store_atomic<T: MetadataValue>(
        &self,
        header: Address,
        val: T,
        optional_mask: Option<T>,
        ordering: Ordering,
    ) {
        self.assert_mask::<T>(optional_mask);
        // SAFETY: `ObjectModel` contract guarantees `self.meta_addr(header)` is a mapped,
        // initialized, writable header cell accessed solely via atomics.
        unsafe { self.resolve_atomic::<T>(header) }.store(val, optional_mask, ordering);
    }

    /// This function provides a default implementation for the `compare_exchange_metadata` method from the `ObjectModel` trait.
    ///
    /// Note: this function only does fetch and exclusive store once, without any busy waiting in a loop.
    pub fn compare_exchange<T: MetadataValue>(
        &self,
        header: Address,
        old_metadata: T,
        new_metadata: T,
        optional_mask: Option<T>,
        success_order: Ordering,
        failure_order: Ordering,
    ) -> Result<T, T> {
        self.assert_mask::<T>(optional_mask);
        // SAFETY: `ObjectModel` contract guarantees `self.meta_addr(header)` is a mapped,
        // initialized, writable header cell accessed solely via atomics.
        unsafe { self.resolve_atomic::<T>(header) }.compare_exchange(
            old_metadata,
            new_metadata,
            optional_mask,
            success_order,
            failure_order,
        )
    }

    /// This function provides a default implementation for the `fetch_add` method from the `ObjectModel` trait.
    pub fn fetch_add<T: MetadataValue>(&self, header: Address, val: T, order: Ordering) -> T {
        // SAFETY: `ObjectModel` contract guarantees `self.meta_addr(header)` is a mapped,
        // initialized, writable header cell accessed solely via atomics.
        unsafe { self.resolve_atomic::<T>(header) }.fetch_add(val, order)
    }

    /// This function provides a default implementation for the `fetch_sub` method from the `ObjectModel` trait.
    pub fn fetch_sub<T: MetadataValue>(&self, header: Address, val: T, order: Ordering) -> T {
        // SAFETY: `ObjectModel` contract guarantees `self.meta_addr(header)` is a mapped,
        // initialized, writable header cell accessed solely via atomics.
        unsafe { self.resolve_atomic::<T>(header) }.fetch_sub(val, order)
    }

    /// This function provides a default implementation for the `fetch_and` method from the `ObjectModel` trait.
    pub fn fetch_and<T: MetadataValue>(&self, header: Address, val: T, order: Ordering) -> T {
        // SAFETY: `ObjectModel` contract guarantees `self.meta_addr(header)` is a mapped,
        // initialized, writable header cell accessed solely via atomics.
        unsafe { self.resolve_atomic::<T>(header) }.fetch_and(val, order)
    }

    /// This function provides a default implementation for the `fetch_or` method from the `ObjectModel` trait.
    pub fn fetch_or<T: MetadataValue>(&self, header: Address, val: T, order: Ordering) -> T {
        // SAFETY: `ObjectModel` contract guarantees `self.meta_addr(header)` is a mapped,
        // initialized, writable header cell accessed solely via atomics.
        unsafe { self.resolve_atomic::<T>(header) }.fetch_or(val, order)
    }

    /// This function provides a default implementation for the `fetch_update` method from the `ObjectModel` trait.
    /// The semantics is the same as Rust's `fetch_update` on atomic types.
    pub fn fetch_update<T: MetadataValue, F: FnMut(T) -> Option<T> + Copy>(
        &self,
        header: Address,
        set_order: Ordering,
        fetch_order: Ordering,
        f: F,
    ) -> std::result::Result<T, T> {
        // SAFETY: `ObjectModel` contract guarantees `self.meta_addr(header)` is a mapped,
        // initialized, writable header cell accessed solely via atomics across `fetch_update`.
        unsafe { self.resolve_atomic::<T>(header) }.fetch_update(set_order, fetch_order, f)
    }
}

impl fmt::Debug for HeaderMetadataSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!(
            "HeaderMetadataSpec {{ \
            **bit_offset: 0x{:x} \
            **num_of_bits: 0x{:x} \
            }}",
            self.bit_offset, self.num_of_bits
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::address::Address;

    #[test]
    fn test_valid_specs() {
        let spec = HeaderMetadataSpec {
            bit_offset: 0,
            num_of_bits: 1,
        };
        spec.assert_spec::<u8>();

        let spec = HeaderMetadataSpec {
            bit_offset: 99,
            num_of_bits: 1,
        };
        spec.assert_spec::<u8>();

        let spec = HeaderMetadataSpec {
            bit_offset: 0,
            num_of_bits: 8,
        };
        spec.assert_spec::<u8>();

        let spec = HeaderMetadataSpec {
            bit_offset: 8,
            num_of_bits: 8,
        };
        spec.assert_spec::<u8>();

        let spec = HeaderMetadataSpec {
            bit_offset: 32,
            num_of_bits: 8,
        };
        spec.assert_spec::<u8>();
    }

    #[test]
    #[should_panic]
    fn test_spec_at_unaligned_offset() {
        let spec = HeaderMetadataSpec {
            bit_offset: 8,
            num_of_bits: 16,
        };
        spec.assert_spec::<u16>();
    }

    #[test]
    #[should_panic]
    fn test_bits_spec_across_byte() {
        // bits across byte boundary
        let spec = HeaderMetadataSpec {
            bit_offset: 7,
            num_of_bits: 2,
        };
        spec.assert_spec::<u8>();
    }

    #[test]
    fn test_negative_bit_offset() {
        let spec = HeaderMetadataSpec {
            bit_offset: -1,
            num_of_bits: 1,
        };
        spec.assert_spec::<u8>();
        assert_eq!(spec.get_shift_and_mask_for_bits(), (7, 0b1000_0000));
        assert_eq!(spec.byte_offset(), -1);
        assert_eq!(spec.get_bits_from_u8(0b1000_0000), 1);
        assert_eq!(spec.get_bits_from_u8(0b0111_1111), 0);

        let spec = HeaderMetadataSpec {
            bit_offset: -2,
            num_of_bits: 1,
        };
        spec.assert_spec::<u8>();
        assert_eq!(spec.get_shift_and_mask_for_bits(), (6, 0b0100_0000));
        assert_eq!(spec.byte_offset(), -1);
        assert_eq!(spec.get_bits_from_u8(0b0100_0000), 1);
        assert_eq!(spec.get_bits_from_u8(0b1011_1111), 0);

        let spec = HeaderMetadataSpec {
            bit_offset: -7,
            num_of_bits: 1,
        };
        spec.assert_spec::<u8>();
        assert_eq!(spec.get_shift_and_mask_for_bits(), (1, 0b0000_0010));
        assert_eq!(spec.byte_offset(), -1);
        assert_eq!(spec.get_bits_from_u8(0b0000_0010), 1);
        assert_eq!(spec.get_bits_from_u8(0b1111_1101), 0);

        let spec = HeaderMetadataSpec {
            bit_offset: -8,
            num_of_bits: 1,
        };
        spec.assert_spec::<u8>();
        assert_eq!(spec.get_shift_and_mask_for_bits(), (0, 0b0000_0001));
        assert_eq!(spec.byte_offset(), -1);
        assert_eq!(spec.get_bits_from_u8(0b0000_0001), 1);
        assert_eq!(spec.get_bits_from_u8(0b1111_1110), 0);

        let spec = HeaderMetadataSpec {
            bit_offset: -9,
            num_of_bits: 1,
        };
        spec.assert_spec::<u8>();
        assert_eq!(spec.get_shift_and_mask_for_bits(), (7, 0b1000_0000));
        assert_eq!(spec.byte_offset(), -2);
        assert_eq!(spec.get_bits_from_u8(0b1000_0000), 1);
        assert_eq!(spec.get_bits_from_u8(0b0111_1111), 0);
    }

    #[test]
    fn test_get_bits_from_u8() {
        // 1 bit
        let spec = HeaderMetadataSpec {
            bit_offset: 0,
            num_of_bits: 1,
        };
        assert_eq!(spec.get_shift_and_mask_for_bits(), (0, 0b1));
        assert_eq!(spec.byte_offset(), 0);
        assert_eq!(spec.get_bits_from_u8(0b0000_0001), 1);
        assert_eq!(spec.get_bits_from_u8(0b1111_1110), 0);

        let spec = HeaderMetadataSpec {
            bit_offset: 1,
            num_of_bits: 1,
        };
        assert_eq!(spec.get_shift_and_mask_for_bits(), (1, 0b10));
        assert_eq!(spec.get_bits_from_u8(0b0000_0010), 1);
        assert_eq!(spec.get_bits_from_u8(0b1111_1101), 0);

        let spec = HeaderMetadataSpec {
            bit_offset: 7,
            num_of_bits: 1,
        };
        assert_eq!(spec.get_shift_and_mask_for_bits(), (7, 0b1000_0000));
        assert_eq!(spec.get_bits_from_u8(0b1000_0000), 1);
        assert_eq!(spec.get_bits_from_u8(0b0111_1111), 0);

        // 1 bit in the next byte
        let spec = HeaderMetadataSpec {
            bit_offset: 8,
            num_of_bits: 1,
        };
        assert_eq!(spec.get_shift_and_mask_for_bits(), (0, 0b1));
        assert_eq!(spec.get_bits_from_u8(0b0000_0001), 1);
        assert_eq!(spec.get_bits_from_u8(0b1111_1110), 0);

        // 2 bits
        let spec = HeaderMetadataSpec {
            bit_offset: 0,
            num_of_bits: 2,
        };
        assert_eq!(spec.get_shift_and_mask_for_bits(), (0, 0b11));
        assert_eq!(spec.get_bits_from_u8(0b0000_0011), 0b11);
        assert_eq!(spec.get_bits_from_u8(0b0000_0010), 0b10);
        assert_eq!(spec.get_bits_from_u8(0b1111_1110), 0b10);

        let spec = HeaderMetadataSpec {
            bit_offset: 6,
            num_of_bits: 2,
        };
        assert_eq!(spec.get_shift_and_mask_for_bits(), (6, 0b1100_0000));
        assert_eq!(spec.get_bits_from_u8(0b1100_0000), 0b11);
        assert_eq!(spec.get_bits_from_u8(0b1000_0000), 0b10);
        assert_eq!(spec.get_bits_from_u8(0b1011_1111), 0b10);

        // 2 bits in the next byte
        let spec = HeaderMetadataSpec {
            bit_offset: 8,
            num_of_bits: 2,
        };
        assert_eq!(spec.get_shift_and_mask_for_bits(), (0, 0b0000_0011));
        assert_eq!(spec.get_bits_from_u8(0b0000_0011), 0b11);
        assert_eq!(spec.get_bits_from_u8(0b0000_0010), 0b10);
        assert_eq!(spec.get_bits_from_u8(0b1111_1110), 0b10);
    }

    #[test]
    fn test_set_bits_to_u8() {
        // 1 bit
        let spec = HeaderMetadataSpec {
            bit_offset: 0,
            num_of_bits: 1,
        };
        assert_eq!(spec.set_bits_to_u8(0b0000_0000, 1), 0b0000_0001);
        assert_eq!(spec.set_bits_to_u8(0b1111_1111, 1), 0b1111_1111);
        assert_eq!(spec.set_bits_to_u8(0b1111_1111, 0), 0b1111_1110);

        let spec = HeaderMetadataSpec {
            bit_offset: 1,
            num_of_bits: 1,
        };
        assert_eq!(spec.set_bits_to_u8(0b0000_0000, 1), 0b0000_0010);
        assert_eq!(spec.set_bits_to_u8(0b1111_1111, 1), 0b1111_1111);
        assert_eq!(spec.set_bits_to_u8(0b1111_1111, 0), 0b1111_1101);

        // 2 bit
        let spec = HeaderMetadataSpec {
            bit_offset: 0,
            num_of_bits: 2,
        };
        assert_eq!(spec.set_bits_to_u8(0b0000_0000, 0b11), 0b0000_0011);
        assert_eq!(spec.set_bits_to_u8(0b1111_1111, 0b11), 0b1111_1111);
        assert_eq!(spec.set_bits_to_u8(0b1111_1111, 0b10), 0b1111_1110);
        assert_eq!(spec.set_bits_to_u8(0b1111_1111, 0b01), 0b1111_1101);
        assert_eq!(spec.set_bits_to_u8(0b1111_1111, 0b00), 0b1111_1100);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic]
    fn test_set_bits_to_u8_exceeds_bits() {
        let spec = HeaderMetadataSpec {
            bit_offset: 0,
            num_of_bits: 1,
        };
        spec.set_bits_to_u8(0, 0b11);
    }

    use paste::paste;

    /// Stack-allocated 3-cell test header buffer (`[prev, curr, next]`) aligned to 8 bytes so even
    /// 64-bit atomics on 32-bit targets satisfy alignment. `with_mid_addr` derives the middle
    /// element's `Address` from `cells.as_mut_ptr()` so raw provenance covers all 3 elements
    /// (`cells[0..3]`, offsets `-size_of::<T>()` through `+2 * size_of::<T>()`) without any live
    /// aliasing Rust reference during `f`.
    #[repr(C, align(8))]
    struct TestHeaderObject<T: MetadataValue + Default> {
        cells: [T; 3],
    }

    impl<T: MetadataValue + Default> TestHeaderObject<T> {
        fn new() -> Self {
            Self {
                cells: [T::default(); 3],
            }
        }

        fn assert_in_bounds(&self, spec: &HeaderMetadataSpec) {
            spec.assert_spec::<T>();
            let cell_bytes = core::mem::size_of::<T>();
            let total_bytes = 3 * cell_bytes;
            let width = if spec.num_of_bits < 8 { 1 } else { cell_bytes };
            let start = (cell_bytes as isize)
                .checked_add(spec.byte_offset())
                .expect("header byte offset overflow");
            assert!(
                start >= 0 && (start as usize) + width <= total_bytes,
                "spec {:?} out of bounds for 3-cell TestHeaderObject ({} bytes)",
                spec,
                total_bytes
            );
        }

        fn with_mid_addr<R>(&mut self, f: impl FnOnce(Address) -> R) -> R {
            let base_ptr = self.cells.as_mut_ptr();
            let mid_addr = Address::from_mut_ptr(base_ptr) + core::mem::size_of::<T>();
            f(mid_addr)
        }

        fn load(&mut self, spec: &HeaderMetadataSpec, mask: Option<T>) -> T {
            self.assert_in_bounds(spec);
            self.with_mid_addr(|obj| {
                // SAFETY: `assert_in_bounds` verified `spec` accesses initialized bytes within
                // `self.cells[0..3]` at valid alignment for `T` (`TestHeaderObject` is 8-byte
                // aligned), and `&mut self` guarantees exclusive access without aliasing references.
                unsafe { spec.load::<T>(obj, mask) }
            })
        }

        fn store(&mut self, spec: &HeaderMetadataSpec, val: T, mask: Option<T>) {
            self.assert_in_bounds(spec);
            self.with_mid_addr(|obj| {
                // SAFETY: `assert_in_bounds` verified `spec` accesses writable, initialized bytes
                // within `self.cells[0..3]` at valid alignment for `T`, and `&mut self` guarantees
                // exclusive access without aliasing references.
                unsafe { spec.store::<T>(obj, val, mask) }
            });
        }
    }

    fn max_value(n_bits: usize) -> u64 {
        (0..n_bits).fold(0, |accum, x| accum + (1 << x))
    }

    macro_rules! test_header_metadata_access {
        ($tname: ident, $type: ty, $num_of_bits: expr) => {
            paste! {
                #[test]
                fn [<$tname _load>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    let spec = HeaderMetadataSpec { bit_offset: 0, num_of_bits: $num_of_bits };
                    assert_eq!(buf.load(&spec, None), 0);
                    let max_value = max_value($num_of_bits) as $type;
                    buf.cells[1] = max_value;
                    assert_eq!(buf.load(&spec, None), max_value);
                }

                #[test]
                fn [<$tname _load_atomic>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    let spec = HeaderMetadataSpec { bit_offset: 0, num_of_bits: $num_of_bits };
                    buf.with_mid_addr(|obj| {
                        assert_eq!(spec.load_atomic::<$type>(obj, None, Ordering::SeqCst), 0);
                    });
                    let max_value = max_value($num_of_bits) as $type;
                    buf.cells[1] = max_value;
                    buf.with_mid_addr(|obj| {
                        assert_eq!(spec.load_atomic::<$type>(obj, None, Ordering::SeqCst), max_value);
                    });
                }

                #[test]
                fn [<$tname _load_next>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    let spec = HeaderMetadataSpec { bit_offset: $num_of_bits, num_of_bits: $num_of_bits };
                    assert_eq!(buf.load(&spec, None), 0);
                    let max_value = max_value($num_of_bits) as $type;
                    if $num_of_bits < 8 {
                        buf.cells[1] = max_value << spec.bit_offset;
                    } else {
                        buf.cells[2] = max_value;
                    }
                    assert_eq!(buf.load(&spec, None), max_value);
                }

                #[test]
                fn [<$tname _load_prev>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    let spec = HeaderMetadataSpec { bit_offset: -$num_of_bits, num_of_bits: $num_of_bits };
                    assert_eq!(buf.load(&spec, None), 0);
                    let max_value = max_value($num_of_bits) as $type;
                    if $num_of_bits < 8 {
                        buf.cells[0] = max_value << (BITS_IN_BYTE as isize + spec.bit_offset);
                    } else {
                        buf.cells[0] = max_value;
                    }
                    assert_eq!(buf.load(&spec, None), max_value);
                }

                #[test]
                fn [<$tname _load_mask>]() {
                    if $num_of_bits < 8 {
                        return;
                    }

                    let mut buf = TestHeaderObject::<$type>::new();
                    let spec = HeaderMetadataSpec { bit_offset: 0, num_of_bits: $num_of_bits };
                    assert_eq!(buf.load(&spec, None), 0);
                    let max_value = max_value($num_of_bits) as $type;
                    buf.cells[1] = max_value;
                    assert_eq!(buf.load(&spec, None), max_value);
                    assert_eq!(buf.load(&spec, Some(0)), 0);
                    assert_eq!(buf.load(&spec, Some(0b101)), 0b101);
                }

                #[test]
                fn [<$tname _store>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    let spec = HeaderMetadataSpec { bit_offset: 0, num_of_bits: $num_of_bits };
                    let max_value = max_value($num_of_bits) as $type;
                    assert_eq!(buf.load(&spec, None), 0);
                    buf.store(&spec, max_value, None);
                    assert_eq!(buf.load(&spec, None), max_value);
                    assert_eq!(buf.cells[1], max_value);
                }

                #[test]
                fn [<$tname _store_atomic>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    let spec = HeaderMetadataSpec { bit_offset: 0, num_of_bits: $num_of_bits };
                    let max_value = max_value($num_of_bits) as $type;
                    assert_eq!(buf.load(&spec, None), 0);
                    buf.with_mid_addr(|obj| {
                        spec.store_atomic::<$type>(obj, max_value, None, Ordering::SeqCst);
                    });
                    assert_eq!(buf.load(&spec, None), max_value);
                    assert_eq!(buf.cells[1], max_value);
                }

                #[test]
                fn [<$tname _store_next>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    let spec = HeaderMetadataSpec { bit_offset: $num_of_bits, num_of_bits: $num_of_bits };
                    let max_value = max_value($num_of_bits) as $type;
                    assert_eq!(buf.load(&spec, None), 0);
                    buf.store(&spec, max_value, None);
                    assert_eq!(buf.load(&spec, None), max_value);
                    if $num_of_bits < 8 {
                        assert_eq!(buf.cells[1], max_value << spec.bit_offset);
                    } else {
                        assert_eq!(buf.cells[2], max_value);
                    }
                }

                #[test]
                fn [<$tname _store_prev>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    let spec = HeaderMetadataSpec { bit_offset: -$num_of_bits, num_of_bits: $num_of_bits };
                    let max_value = max_value($num_of_bits) as $type;
                    assert_eq!(buf.load(&spec, None), 0);
                    buf.store(&spec, max_value, None);
                    assert_eq!(buf.load(&spec, None), max_value);
                    if $num_of_bits < 8 {
                        assert_eq!(buf.cells[0], max_value << (BITS_IN_BYTE as isize + spec.bit_offset));
                    } else {
                        assert_eq!(buf.cells[0], max_value);
                    }
                }

                #[test]
                fn [<$tname _store_mask>]() {
                    if $num_of_bits < 8 {
                        return;
                    }

                    let mut buf = TestHeaderObject::<$type>::new();
                    let spec = HeaderMetadataSpec { bit_offset: 0, num_of_bits: $num_of_bits };
                    let max_value = max_value($num_of_bits) as $type;
                    assert_eq!(buf.load(&spec, None), 0);

                    buf.store(&spec, max_value, Some(max_value));
                    assert_eq!(buf.load(&spec, None), max_value);

                    buf.store(&spec, 0, None);
                    buf.store(&spec, max_value, Some(0b10));
                    assert_eq!(buf.load(&spec, None), 0b10);
                    assert_eq!(buf.cells[1], 0b10);
                }

                #[test]
                fn [<$tname _compare_exchange_success>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    let spec = HeaderMetadataSpec { bit_offset: 0, num_of_bits: $num_of_bits };
                    let old_val = buf.load(&spec, None);
                    assert_eq!(old_val, 0);

                    let max_value = max_value($num_of_bits) as $type;
                    let res = buf.with_mid_addr(|obj| {
                        spec.compare_exchange::<$type>(obj, old_val, max_value, None, Ordering::SeqCst, Ordering::SeqCst)
                    });
                    assert_eq!(res, Ok(old_val));
                    assert_eq!(buf.load(&spec, None), max_value);
                }

                #[test]
                fn [<$tname _compare_exchange_fail>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    let spec = HeaderMetadataSpec { bit_offset: 0, num_of_bits: $num_of_bits };
                    let old_val = buf.load(&spec, None);
                    assert_eq!(old_val, 0);

                    buf.store(&spec, 1, None);

                    let max_value = max_value($num_of_bits) as $type;
                    let res = buf.with_mid_addr(|obj| {
                        spec.compare_exchange::<$type>(obj, old_val, max_value, None, Ordering::SeqCst, Ordering::SeqCst)
                    });
                    assert_eq!(res, Err(1));
                    assert_eq!(buf.load(&spec, None), 1);
                }

                #[test]
                fn [<$tname _fetch_add>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    for bit_offset in (0isize..($type::BITS as isize)).step_by($num_of_bits) {
                        let spec = HeaderMetadataSpec { bit_offset, num_of_bits: $num_of_bits };
                        let max_value = max_value($num_of_bits) as $type;

                        let old_val = buf.load(&spec, None);
                        assert_eq!(old_val, 0);

                        let old_val_from_fetch = buf.with_mid_addr(|obj| {
                            spec.fetch_add::<$type>(obj, max_value, Ordering::SeqCst)
                        });
                        assert_eq!(old_val, old_val_from_fetch);
                        assert_eq!(buf.load(&spec, None), max_value);
                    }
                }

                #[test]
                fn [<$tname _fetch_add_overflow>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    for bit_offset in (0isize..($type::BITS as isize)).step_by($num_of_bits) {
                        let spec = HeaderMetadataSpec { bit_offset, num_of_bits: $num_of_bits };
                        let max_value = max_value($num_of_bits) as $type;

                        buf.store(&spec, max_value, None);
                        let old_val = buf.load(&spec, None);

                        // add 1 will cause overflow
                        let old_val_from_fetch = buf.with_mid_addr(|obj| {
                            spec.fetch_add::<$type>(obj, 1, Ordering::SeqCst)
                        });
                        assert_eq!(old_val, old_val_from_fetch);
                        assert_eq!(buf.load(&spec, None), 0);
                        assert_eq!(buf.cells[1], 0); // we should not accidentally affect other bits
                    }
                }

                #[test]
                fn [<$tname _fetch_sub>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    for bit_offset in (0isize..($type::BITS as isize)).step_by($num_of_bits) {
                        let spec = HeaderMetadataSpec { bit_offset, num_of_bits: $num_of_bits };

                        buf.store(&spec, 1, None);
                        let old_val = buf.load(&spec, None);
                        assert_eq!(old_val, 1);

                        let old_val_from_fetch = buf.with_mid_addr(|obj| {
                            spec.fetch_sub::<$type>(obj, 1, Ordering::SeqCst)
                        });
                        assert_eq!(old_val, old_val_from_fetch);
                        assert_eq!(buf.load(&spec, None), 0);
                    }
                }

                #[test]
                fn [<$tname _fetch_sub_overflow>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    for bit_offset in (0isize..($type::BITS as isize)).step_by($num_of_bits) {
                        let spec = HeaderMetadataSpec { bit_offset, num_of_bits: $num_of_bits };
                        let max_value = max_value($num_of_bits) as $type;

                        let old_val = buf.load(&spec, None);
                        assert_eq!(old_val, 0);

                        let old_val_from_fetch = buf.with_mid_addr(|obj| {
                            spec.fetch_sub::<$type>(obj, 1, Ordering::SeqCst)
                        });
                        assert_eq!(old_val, old_val_from_fetch);
                        assert_eq!(buf.load(&spec, None), max_value);
                    }
                }

                #[test]
                fn [<$tname _fetch_and>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    for bit_offset in (0isize..($type::BITS as isize)).step_by($num_of_bits) {
                        let spec = HeaderMetadataSpec { bit_offset, num_of_bits: $num_of_bits };
                        let max_value = max_value($num_of_bits) as $type;

                        let old_val = buf.load(&spec, None);
                        assert_eq!(old_val, 0);

                        let old_val_from_fetch = buf.with_mid_addr(|obj| {
                            spec.fetch_and::<$type>(obj, max_value, Ordering::SeqCst)
                        });
                        assert_eq!(old_val, old_val_from_fetch);
                        assert_eq!(buf.load(&spec, None), 0);
                    }
                }

                #[test]
                fn [<$tname _fetch_or>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    for bit_offset in (0isize..($type::BITS as isize)).step_by($num_of_bits) {
                        let spec = HeaderMetadataSpec { bit_offset, num_of_bits: $num_of_bits };
                        let max_value = max_value($num_of_bits) as $type;

                        let old_val = buf.load(&spec, None);
                        assert_eq!(old_val, 0);

                        let old_val_from_fetch = buf.with_mid_addr(|obj| {
                            spec.fetch_or::<$type>(obj, max_value, Ordering::SeqCst)
                        });
                        assert_eq!(old_val, old_val_from_fetch);
                        assert_eq!(buf.load(&spec, None), max_value);
                    }
                }

                #[test]
                fn [<$tname _fetch_update_success>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    for bit_offset in (0isize..($type::BITS as isize)).step_by($num_of_bits) {
                        let spec = HeaderMetadataSpec { bit_offset, num_of_bits: $num_of_bits };
                        let max_value = max_value($num_of_bits) as $type;

                        let old_val = buf.load(&spec, None);
                        assert_eq!(old_val, 0);

                        let update_res = buf.with_mid_addr(|obj| {
                            spec.fetch_update(obj, Ordering::SeqCst, Ordering::SeqCst, |_x: $type| Some(max_value))
                        });
                        assert_eq!(update_res, Ok(old_val));
                        assert_eq!(buf.load(&spec, None), max_value);
                    }
                }

                #[test]
                fn [<$tname _fetch_update_fail>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    for bit_offset in (0isize..($type::BITS as isize)).step_by($num_of_bits) {
                        let spec = HeaderMetadataSpec { bit_offset, num_of_bits: $num_of_bits };

                        let old_val = buf.load(&spec, None);
                        assert_eq!(old_val, 0);

                        let update_res = buf.with_mid_addr(|obj| {
                            spec.fetch_update(obj, Ordering::SeqCst, Ordering::SeqCst, |_x: $type| None)
                        });
                        assert_eq!(update_res, Err(old_val));
                        assert_eq!(buf.load(&spec, None), 0);
                    }
                }
            }
        }
    }

    test_header_metadata_access!(test_u1, u8, 1);
    test_header_metadata_access!(test_u2, u8, 2);
    test_header_metadata_access!(test_u4, u8, 4);
    test_header_metadata_access!(test_u8, u8, 8);
    test_header_metadata_access!(test_u16, u16, 16);
    test_header_metadata_access!(test_u32, u32, 32);
    test_header_metadata_access!(test_u64, u64, 64);
    test_header_metadata_access!(
        test_usize,
        usize,
        if cfg!(target_pointer_width = "64") {
            64
        } else if cfg!(target_pointer_width = "32") {
            32
        } else {
            unreachable!()
        }
    );

    #[test]
    #[should_panic(expected = "num_of_bits")]
    fn test_spec_width_mismatch_panics() {
        let spec = HeaderMetadataSpec {
            bit_offset: 0,
            num_of_bits: 8,
        };
        spec.assert_spec::<u64>();
    }

    #[test]
    fn test_release_acqrel_orderings_on_subbyte_and_masked_specs() {
        // Sub-byte 2-bit spec exercising `Release` / `AcqRel` emulated CAS/RMW paths while
        // preserving adjacent bits in the same byte (`0b1000_0001`).
        let mut sub_buf = TestHeaderObject::<u8>::new();
        sub_buf.cells[1] = 0b1000_0001;
        let sub_spec = HeaderMetadataSpec {
            bit_offset: 2,
            num_of_bits: 2,
        };
        sub_buf.with_mid_addr(|obj| {
            sub_spec.store_atomic::<u8>(obj, 0b10, None, Ordering::Release);
            assert_eq!(
                sub_spec.load_atomic::<u8>(obj, None, Ordering::Acquire),
                0b10
            );

            assert_eq!(
                sub_spec.compare_exchange::<u8>(
                    obj,
                    0b10,
                    0b01,
                    None,
                    Ordering::Release,
                    Ordering::Relaxed
                ),
                Ok(0b10)
            );
            assert_eq!(
                sub_spec.compare_exchange::<u8>(
                    obj,
                    0b10,
                    0b11,
                    None,
                    Ordering::AcqRel,
                    Ordering::Acquire
                ),
                Err(0b01)
            );
            assert_eq!(
                sub_spec.compare_exchange::<u8>(
                    obj,
                    0b01,
                    0b10,
                    None,
                    Ordering::AcqRel,
                    Ordering::Acquire
                ),
                Ok(0b01)
            );

            assert_eq!(sub_spec.fetch_add::<u8>(obj, 1, Ordering::Release), 0b10);
            assert_eq!(sub_spec.fetch_add::<u8>(obj, 1, Ordering::AcqRel), 0b11);
            assert_eq!(sub_spec.fetch_sub::<u8>(obj, 1, Ordering::Release), 0b00);
            assert_eq!(sub_spec.fetch_sub::<u8>(obj, 1, Ordering::AcqRel), 0b11);
            assert_eq!(
                sub_spec.load_atomic::<u8>(obj, None, Ordering::Acquire),
                0b10
            );
        });
        assert_eq!(sub_buf.cells[1], 0b1000_0001 | (0b10 << 2));

        // Masked 16-bit spec exercising `Release` / `AcqRel` masked CAS/RMW paths while preserving
        // unmasked low bits (`0b11`).
        let mut word_buf = TestHeaderObject::<u16>::new();
        word_buf.cells[1] = 0b11;
        let word_spec = HeaderMetadataSpec {
            bit_offset: 0,
            num_of_bits: 16,
        };
        let mask: u16 = !0b11u16;
        word_buf.with_mid_addr(|obj| {
            word_spec.store_atomic::<u16>(obj, 0x1230, Some(mask), Ordering::Release);
            assert_eq!(
                word_spec.load_atomic::<u16>(obj, Some(mask), Ordering::Acquire),
                0x1230
            );
            assert_eq!(
                word_spec.load_atomic::<u16>(obj, None, Ordering::Acquire),
                0x1233
            );

            assert_eq!(
                word_spec.compare_exchange::<u16>(
                    obj,
                    0x1230,
                    0x4560,
                    Some(mask),
                    Ordering::Release,
                    Ordering::Relaxed
                ),
                Ok(0x1233)
            );
            assert_eq!(
                word_spec.compare_exchange::<u16>(
                    obj,
                    0x4560,
                    0x7890,
                    Some(mask),
                    Ordering::AcqRel,
                    Ordering::Acquire
                ),
                Ok(0x4563)
            );
            assert_eq!(
                word_spec.load_atomic::<u16>(obj, None, Ordering::Acquire),
                0x7893
            );
        });
    }

    #[test]
    fn test_masked_compare_exchange_ignores_and_preserves_unmasked_bits() {
        let mut buf = TestHeaderObject::<u16>::new();
        // Low 2 bits (`0b10`) belong to an adjacent unmasked field; upper 14 bits are masked.
        buf.cells[1] = 0x1232;
        let spec = HeaderMetadataSpec {
            bit_offset: 0,
            num_of_bits: 16,
        };
        let mask: u16 = !0b11u16;
        buf.with_mid_addr(|obj| {
            // `old_val` (`0x1231`) and `new_val` (`0x4563`) both carry non-zero bits outside `mask`:
            // CAS must still match on masked `0x1230` and must preserve adjacent `0b10` untouched.
            assert_eq!(
                spec.compare_exchange::<u16>(
                    obj,
                    0x1231,
                    0x4563,
                    Some(mask),
                    Ordering::AcqRel,
                    Ordering::Acquire
                ),
                Ok(0x1232)
            );
            assert_eq!(
                spec.load_atomic::<u16>(obj, None, Ordering::Acquire),
                0x4562
            );

            // Genuine mismatch in masked bits must fail even if unmasked bits happen to match.
            assert_eq!(
                spec.compare_exchange::<u16>(
                    obj,
                    0x9992,
                    0x7893,
                    Some(mask),
                    Ordering::AcqRel,
                    Ordering::Acquire
                ),
                Err(0x4562)
            );
            assert_eq!(
                spec.load_atomic::<u16>(obj, None, Ordering::Acquire),
                0x4562
            );
        });
    }
}
