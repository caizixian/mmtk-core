//! This module provides a default implementation of the access functions for in-header metadata.

use atomic::Ordering;
use std::fmt;

use crate::util::constants::{BITS_IN_BYTE, LOG_BITS_IN_BYTE};
use crate::util::metadata::metadata_val_traits::*;
use crate::util::Address;
use num_traits::FromPrimitive;

#[allow(dead_code)]
const LOG_BITS_IN_U16: usize = 4;
#[allow(dead_code)]
const BITS_IN_U16: usize = 1 << LOG_BITS_IN_U16;
#[allow(dead_code)]
const LOG_BITS_IN_U32: usize = 5;
#[allow(dead_code)]
const BITS_IN_U32: usize = 1 << LOG_BITS_IN_U32;
#[allow(dead_code)]
const LOG_BITS_IN_U64: usize = 6;
#[allow(dead_code)]
const BITS_IN_U64: usize = 1 << LOG_BITS_IN_U64;

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
    #[cfg(debug_assertions)]
    fn assert_mask<T: MetadataValue>(&self, mask: Option<T>) {
        debug_assert!(mask.is_none() || self.num_of_bits >= 8, "optional_mask is only supported for 8X-bits in-header metadata. Problematic MetadataSpec: ({:?})", self);
    }

    /// Assert if this is a valid spec.
    #[cfg(debug_assertions)]
    fn assert_spec<T: MetadataValue>(&self) {
        if self.num_of_bits == 0 {
            panic!("Metadata of 0 bits is not allowed.");
        } else if self.num_of_bits < 8 {
            debug_assert!(
                (self.bit_offset >> LOG_BITS_IN_BYTE)
                    == ((self.bit_offset + self.num_of_bits as isize - 1) >> LOG_BITS_IN_BYTE),
                "Metadata << 8-bits: ({:?}) stretches over two bytes!",
                self
            );
        } else if self.num_of_bits >= 8 && self.num_of_bits <= 64 {
            debug_assert!(
                self.bit_offset.trailing_zeros() >= T::LOG2,
                "{:?}: bit_offset must be aligned to {}",
                self,
                1 << T::LOG2
            );
        } else {
            // num_of_bits larger than 64
            unreachable!("Metadata that is larger than 64-bits is not supported")
        }
    }

    fn byte_offset(&self) -> isize {
        self.bit_offset >> LOG_BITS_IN_BYTE
    }

    fn meta_addr(&self, header: Address) -> Address {
        header + self.byte_offset()
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

    /// Truncate a value based on the spec.
    fn truncate_bits_in_u8(&self, val: u8) -> u8 {
        debug_assert!(self.num_of_bits < BITS_IN_BYTE);
        val & ((1 << self.num_of_bits) - 1)
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
        #[cfg(debug_assertions)]
        {
            self.assert_mask::<T>(optional_mask);
            self.assert_spec::<T>();
        }

        let addr = self.meta_addr(header);
        // Metadata smaller than 8-bits is packed into a single byte and requires shift and mask.
        let res: T = if self.num_of_bits < 8 {
            // SAFETY: Caller guarantees `addr` (`header + byte_offset`) is non-null, points to an
            // initialized header byte (`u8` has alignment 1), and has no racing writes or `&mut` aliases.
            let byte_val = unsafe { <u8 as MetadataValue>::load(addr) };
            FromPrimitive::from_u8(self.get_bits_from_u8(byte_val)).unwrap()
        } else {
            // SAFETY: Caller guarantees `addr` is non-null, aligned to `T` (verified in debug builds
            // by `assert_spec`), points to initialized header memory for `size_of::<T>()` bytes, and
            // has no racing writes or `&mut` aliases.
            unsafe { T::load(addr) }
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
        #[cfg(debug_assertions)]
        {
            self.assert_mask::<T>(optional_mask);
            self.assert_spec::<T>();
        }

        let addr = self.meta_addr(header);
        // Metadata smaller than 8-bits is special in that more than one metadata value may be
        // included in one AtomicU8 operation, and extra shift and mask is required.
        let res: T = if self.num_of_bits < 8 {
            // SAFETY: By MMTk's `ObjectModel` header-metadata contract (preserved as a `pub` API for
            // VM bindings), `header` is a live object's header base address, so `addr` is non-null,
            // 1-byte aligned for `AtomicU8`, and points to mapped, initialized header memory accessed
            // atomically without conflicting non-atomic writes or `&mut` references.
            let byte_val = unsafe { <u8 as MetadataValue>::load_atomic(addr, ordering) };
            FromPrimitive::from_u8(self.get_bits_from_u8(byte_val)).unwrap()
        } else {
            // SAFETY: By MMTk's `ObjectModel` header-metadata contract and `self`'s alignment
            // (`bit_offset` aligned to `T::BITS`, checked by `assert_spec`), `addr` is non-null,
            // aligned to `T`'s atomic counterpart, and points to mapped, initialized header memory
            // of `size_of::<T>()` bytes with no conflicting non-atomic accesses or `&mut` aliases.
            unsafe { T::load_atomic(addr, ordering) }
        };

        if let Some(mask) = optional_mask {
            res.bitand(mask)
        } else {
            res
        }
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
        #[cfg(debug_assertions)]
        {
            self.assert_mask::<T>(optional_mask);
            self.assert_spec::<T>();
        }

        if self.num_of_bits < 8 {
            let val_u8 = val.to_u8().unwrap();
            let byte_addr = self.meta_addr(header);
            // SAFETY: Caller guarantees `byte_addr` is non-null, points to an initialized writable
            // header byte, and is exclusively accessed without concurrent readers, writers, or
            // aliasing Rust references.
            let old_byte_val = unsafe { <u8 as MetadataValue>::load(byte_addr) };
            let new_byte_val = self.set_bits_to_u8(old_byte_val, val_u8);
            // SAFETY: Same exclusive, non-null, 1-byte-aligned header byte as loaded above.
            unsafe {
                <u8 as MetadataValue>::store(byte_addr, new_byte_val);
            }
        } else {
            let addr = self.meta_addr(header);
            let val = if let Some(mask) = optional_mask {
                // SAFETY: Caller guarantees `addr` is non-null, aligned to `T`, initialized, and
                // exclusively accessed without concurrent accesses or aliasing Rust references.
                let old_val = unsafe { T::load(addr) };
                old_val.bitand(mask.inv()).bitor(val.bitand(mask))
            } else {
                val
            };
            // SAFETY: Caller guarantees `addr` is non-null, aligned to `T`, writable for
            // `size_of::<T>()` bytes, and exclusively accessed without aliasing references.
            unsafe {
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
        #[cfg(debug_assertions)]
        {
            self.assert_mask::<T>(optional_mask);
            self.assert_spec::<T>();
        }

        // Metadata smaller than 8-bits is special in that more than one metadata value may be
        // included in one AtomicU8 operation, requiring shift, mask, and `fetch_update`.
        if self.num_of_bits < 8 {
            let val_u8 = val.to_u8().unwrap();
            let byte_addr = self.meta_addr(header);
            // SAFETY: By MMTk's `ObjectModel` header-metadata contract, `byte_addr` is non-null,
            // 1-byte aligned for `AtomicU8`, and points to mapped, initialized, writable header
            // memory with no conflicting non-atomic accesses or non-atomic Rust references.
            let _ = unsafe {
                <u8 as MetadataValue>::fetch_update(byte_addr, ordering, ordering, |old_val: u8| {
                    Some(self.set_bits_to_u8(old_val, val_u8))
                })
            };
        } else {
            let addr = self.meta_addr(header);
            // If the optional mask is provided (e.g. for forwarding pointer), we need compare-exchange.
            if let Some(mask) = optional_mask {
                // SAFETY: By MMTk's `ObjectModel` header-metadata contract and `self`'s natural
                // alignment (`assert_spec`), `addr` is non-null, aligned to `T`'s atomic counterpart,
                // and points to mapped, initialized, writable header memory without non-atomic races.
                let _ = unsafe {
                    T::fetch_update(addr, ordering, ordering, |old_val: T| {
                        Some(old_val.bitand(mask.inv()).bitor(val.bitand(mask)))
                    })
                };
            } else {
                // SAFETY: By MMTk's `ObjectModel` header-metadata contract and `self`'s natural
                // alignment (`assert_spec`), `addr` is non-null, aligned to `T`'s atomic counterpart,
                // and points to mapped, initialized, writable header memory without non-atomic races.
                unsafe {
                    T::store_atomic(addr, val, ordering);
                }
            }
        }
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
        #[cfg(debug_assertions)]
        self.assert_spec::<T>();
        // Metadata smaller than 8-bits is special in that more than one metadata value may be
        // included in one AtomicU8 operation, and extra shift and mask is required.
        if self.num_of_bits < 8 {
            let byte_addr = self.meta_addr(header);
            // SAFETY: By MMTk's `ObjectModel` header-metadata contract, `byte_addr` is non-null,
            // 1-byte aligned for `AtomicU8`, and points to mapped, initialized header memory
            // without conflicting non-atomic accesses or `&mut` aliases.
            let real_old_byte =
                unsafe { <u8 as MetadataValue>::load_atomic(byte_addr, success_order) };
            let expected_old_byte =
                self.set_bits_to_u8(real_old_byte, old_metadata.to_u8().unwrap());
            let expected_new_byte =
                self.set_bits_to_u8(expected_old_byte, new_metadata.to_u8().unwrap());
            // SAFETY: `byte_addr` satisfies the same `AtomicU8` validity, initialization, and
            // atomic-only concurrency invariants as the load above.
            unsafe {
                <u8 as MetadataValue>::compare_exchange(
                    byte_addr,
                    expected_old_byte,
                    expected_new_byte,
                    success_order,
                    failure_order,
                )
            }
            .map(|x| FromPrimitive::from_u8(x).unwrap())
            .map_err(|x| FromPrimitive::from_u8(x).unwrap())
        } else {
            let addr = self.meta_addr(header);
            let (old_metadata, new_metadata) = if let Some(mask) = optional_mask {
                // SAFETY: By MMTk's `ObjectModel` header-metadata contract and `self`'s natural
                // alignment (`assert_spec`), `addr` is non-null, aligned to `T`'s atomic type, and
                // points to mapped, initialized header memory without non-atomic data races.
                let old_byte = unsafe { T::load_atomic(addr, success_order) };
                let expected_new_byte = old_byte.bitand(mask.inv()).bitor(new_metadata);
                let expected_old_byte = old_byte.bitand(mask.inv()).bitor(old_metadata);
                (expected_old_byte, expected_new_byte)
            } else {
                (old_metadata, new_metadata)
            };

            // SAFETY: By MMTk's `ObjectModel` header-metadata contract and `self`'s natural
            // alignment (`assert_spec`), `addr` is non-null, aligned to `T`'s atomic type, and
            // points to mapped, initialized, writable header memory without non-atomic races.
            unsafe {
                T::compare_exchange(
                    addr,
                    old_metadata,
                    new_metadata,
                    success_order,
                    failure_order,
                )
            }
        }
    }

    /// Inner method for `fetch_add`/`fetch_sub` on sub-byte (`< 8` bits) header metadata.
    ///
    /// # Safety
    /// * `self.num_of_bits` must be in `1..8` without crossing a byte boundary.
    /// * `self.meta_addr(header)` must be non-null and point to a mapped, initialized, writable
    ///   object-header byte accessed only via atomic operations for the duration of this call.
    /// * `set_order` and `fetch_order` must be valid orderings for `AtomicU8::fetch_update`.
    unsafe fn fetch_ops_on_bits<F: Fn(u8) -> u8>(
        &self,
        header: Address,
        set_order: Ordering,
        fetch_order: Ordering,
        update: F,
    ) -> u8 {
        let byte_addr = self.meta_addr(header);
        // SAFETY: Caller guarantees `byte_addr` is non-null, points to a mapped, initialized,
        // writable header byte (`AtomicU8`, alignment 1) with no conflicting non-atomic accesses,
        // and `set_order`/`fetch_order` are valid for `fetch_update`.
        let old_raw_byte = unsafe {
            <u8 as MetadataValue>::fetch_update(
                byte_addr,
                set_order,
                fetch_order,
                |raw_byte: u8| {
                    let old_metadata = self.get_bits_from_u8(raw_byte);
                    let new_metadata = self.truncate_bits_in_u8(update(old_metadata));
                    let new_byte = self.set_bits_to_u8(raw_byte, new_metadata);
                    Some(new_byte)
                },
            )
        }
        .unwrap();
        self.get_bits_from_u8(old_raw_byte)
    }

    /// This function provides a default implementation for the `fetch_add` method from the `ObjectModel` trait.
    pub fn fetch_add<T: MetadataValue>(&self, header: Address, val: T, order: Ordering) -> T {
        #[cfg(debug_assertions)]
        self.assert_spec::<T>();
        if self.num_of_bits < 8 {
            // SAFETY: `self.num_of_bits < 8` holds, and by MMTk's `ObjectModel` header-metadata
            // contract `self.meta_addr(header)` is a valid, mapped, initialized header byte
            // accessed atomically without non-atomic races.
            let old_bits = unsafe {
                self.fetch_ops_on_bits(header, order, order, |x: u8| {
                    x.wrapping_add(val.to_u8().unwrap())
                })
            };
            FromPrimitive::from_u8(old_bits).unwrap()
        } else {
            // SAFETY: By MMTk's `ObjectModel` header-metadata contract and `self`'s natural
            // alignment (`assert_spec`), `self.meta_addr(header)` is non-null, aligned to `T`'s
            // atomic type, and points to mapped, initialized, writable header memory.
            unsafe { T::fetch_add(self.meta_addr(header), val, order) }
        }
    }

    /// This function provides a default implementation for the `fetch_sub` method from the `ObjectModel` trait.
    pub fn fetch_sub<T: MetadataValue>(&self, header: Address, val: T, order: Ordering) -> T {
        #[cfg(debug_assertions)]
        self.assert_spec::<T>();
        if self.num_of_bits < 8 {
            // SAFETY: `self.num_of_bits < 8` holds, and by MMTk's `ObjectModel` header-metadata
            // contract `self.meta_addr(header)` is a valid, mapped, initialized header byte
            // accessed atomically without non-atomic races.
            let old_bits = unsafe {
                self.fetch_ops_on_bits(header, order, order, |x: u8| {
                    x.wrapping_sub(val.to_u8().unwrap())
                })
            };
            FromPrimitive::from_u8(old_bits).unwrap()
        } else {
            // SAFETY: By MMTk's `ObjectModel` header-metadata contract and `self`'s natural
            // alignment (`assert_spec`), `self.meta_addr(header)` is non-null, aligned to `T`'s
            // atomic type, and points to mapped, initialized, writable header memory.
            unsafe { T::fetch_sub(self.meta_addr(header), val, order) }
        }
    }

    /// This function provides a default implementation for the `fetch_and` method from the `ObjectModel` trait.
    pub fn fetch_and<T: MetadataValue>(&self, header: Address, val: T, order: Ordering) -> T {
        #[cfg(debug_assertions)]
        self.assert_spec::<T>();
        if self.num_of_bits < 8 {
            let (lshift, mask) = self.get_shift_and_mask_for_bits();
            let new_val = (val.to_u8().unwrap() << lshift) | !mask;
            // We do not need to use fetch_ops_on_bits(), we can just set irrelevant bits to 1, and do fetch_and
            // SAFETY: By MMTk's `ObjectModel` header-metadata contract, `self.meta_addr(header)` is
            // non-null, 1-byte aligned for `AtomicU8`, and points to mapped, initialized, writable
            // header memory accessed atomically without non-atomic races.
            let old_raw_byte =
                unsafe { <u8 as MetadataValue>::fetch_and(self.meta_addr(header), new_val, order) };
            let old_val = self.get_bits_from_u8(old_raw_byte);
            FromPrimitive::from_u8(old_val).unwrap()
        } else {
            // SAFETY: By MMTk's `ObjectModel` header-metadata contract and `self`'s natural
            // alignment (`assert_spec`), `self.meta_addr(header)` is non-null, aligned to `T`'s
            // atomic type, and points to mapped, initialized, writable header memory.
            unsafe { T::fetch_and(self.meta_addr(header), val, order) }
        }
    }

    /// This function provides a default implementation for the `fetch_or` method from the `ObjectModel` trait.
    pub fn fetch_or<T: MetadataValue>(&self, header: Address, val: T, order: Ordering) -> T {
        #[cfg(debug_assertions)]
        self.assert_spec::<T>();
        if self.num_of_bits < 8 {
            let (lshift, mask) = self.get_shift_and_mask_for_bits();
            let new_val = (val.to_u8().unwrap() << lshift) & mask;
            // We do not need to use fetch_ops_on_bits(), we can just set irrelevant bits to 0, and do fetch_or
            // SAFETY: By MMTk's `ObjectModel` header-metadata contract, `self.meta_addr(header)` is
            // non-null, 1-byte aligned for `AtomicU8`, and points to mapped, initialized, writable
            // header memory accessed atomically without non-atomic races.
            let old_raw_byte =
                unsafe { <u8 as MetadataValue>::fetch_or(self.meta_addr(header), new_val, order) };
            let old_val = self.get_bits_from_u8(old_raw_byte);
            FromPrimitive::from_u8(old_val).unwrap()
        } else {
            // SAFETY: By MMTk's `ObjectModel` header-metadata contract and `self`'s natural
            // alignment (`assert_spec`), `self.meta_addr(header)` is non-null, aligned to `T`'s
            // atomic type, and points to mapped, initialized, writable header memory.
            unsafe { T::fetch_or(self.meta_addr(header), val, order) }
        }
    }

    /// This function provides a default implementation for the `fetch_update` method from the `ObjectModel` trait.
    /// The semantics is the same as Rust's `fetch_update` on atomic types.
    pub fn fetch_update<T: MetadataValue, F: FnMut(T) -> Option<T> + Copy>(
        &self,
        header: Address,
        set_order: Ordering,
        fetch_order: Ordering,
        mut f: F,
    ) -> std::result::Result<T, T> {
        #[cfg(debug_assertions)]
        self.assert_spec::<T>();
        if self.num_of_bits < 8 {
            let byte_addr = self.meta_addr(header);
            // SAFETY: By MMTk's `ObjectModel` header-metadata contract, `byte_addr` is non-null,
            // 1-byte aligned for `AtomicU8`, and points to mapped, initialized, writable header
            // memory accessed atomically without non-atomic races across the `fetch_update` loop.
            unsafe {
                <u8 as MetadataValue>::fetch_update(
                    byte_addr,
                    set_order,
                    fetch_order,
                    |raw_byte: u8| {
                        let old_metadata = self.get_bits_from_u8(raw_byte);
                        f(FromPrimitive::from_u8(old_metadata).unwrap()).map(|new_val| {
                            let new_metadata = self.truncate_bits_in_u8(new_val.to_u8().unwrap());
                            self.set_bits_to_u8(raw_byte, new_metadata)
                        })
                    },
                )
            }
            .map(|raw_byte| FromPrimitive::from_u8(self.get_bits_from_u8(raw_byte)).unwrap())
            .map_err(|raw_byte| FromPrimitive::from_u8(self.get_bits_from_u8(raw_byte)).unwrap())
        } else {
            // SAFETY: By MMTk's `ObjectModel` header-metadata contract and `self`'s natural
            // alignment (`assert_spec`), `self.meta_addr(header)` is non-null, aligned to `T`'s
            // atomic type, and points to mapped, initialized, writable header memory.
            unsafe { T::fetch_update(self.meta_addr(header), set_order, fetch_order, f) }
        }
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

#[cfg(all(test, debug_assertions))]
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
    struct TestHeaderObject<T: Copy + Default> {
        cells: [T; 3],
    }

    impl<T: Copy + Default> TestHeaderObject<T> {
        fn new() -> Self {
            Self {
                cells: [T::default(); 3],
            }
        }

        fn with_mid_addr<R>(&mut self, f: impl FnOnce(Address) -> R) -> R {
            let base_ptr = self.cells.as_mut_ptr();
            let mid_addr = Address::from_mut_ptr(base_ptr) + core::mem::size_of::<T>();
            f(mid_addr)
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
                    buf.with_mid_addr(|obj| {
                        // SAFETY: `obj` points to `buf.cells[1]`, which is zero-initialized, aligned
                        // to `$type`, and exclusively accessed by this single-threaded test.
                        assert_eq!(unsafe { spec.load::<$type>(obj, None) }, 0);
                    });
                    let max_value = max_value($num_of_bits) as $type;
                    buf.cells[1] = max_value;
                    buf.with_mid_addr(|obj| {
                        // SAFETY: `obj` points to `buf.cells[1]`, initialized above to `max_value`,
                        // aligned to `$type`, with no aliasing references during `with_mid_addr`.
                        assert_eq!(unsafe { spec.load::<$type>(obj, None) }, max_value);
                    });
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
                    buf.with_mid_addr(|obj| {
                        // SAFETY: `obj` has provenance over `buf.cells[0..3]`; `spec` accesses either
                        // `buf.cells[1]` (`$num_of_bits < 8`) or `buf.cells[2]` (`>= 8`), both
                        // zero-initialized, aligned to `$type`, and exclusively accessed here.
                        assert_eq!(unsafe { spec.load::<$type>(obj, None) }, 0);
                    });
                    let max_value = max_value($num_of_bits) as $type;
                    if $num_of_bits < 8 {
                        buf.cells[1] = max_value << spec.bit_offset;
                    } else {
                        buf.cells[2] = max_value;
                    }
                    buf.with_mid_addr(|obj| {
                        // SAFETY: `obj` has provenance over `buf.cells[0..3]`; the target cell is
                        // initialized, aligned to `$type`, and unaliased during `with_mid_addr`.
                        assert_eq!(unsafe { spec.load::<$type>(obj, None) }, max_value);
                    });
                }

                #[test]
                fn [<$tname _load_prev>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    let spec = HeaderMetadataSpec { bit_offset: -$num_of_bits, num_of_bits: $num_of_bits };
                    buf.with_mid_addr(|obj| {
                        // SAFETY: `obj` is derived from `buf.cells.as_mut_ptr()`, so negative offset
                        // `-$num_of_bits` resolves in-bounds to `buf.cells[0]`, which is initialized,
                        // aligned to `$type`, and unaliased during `with_mid_addr`.
                        assert_eq!(unsafe { spec.load::<$type>(obj, None) }, 0);
                    });
                    let max_value = max_value($num_of_bits) as $type;
                    if $num_of_bits < 8 {
                        buf.cells[0] = max_value << (BITS_IN_BYTE as isize + spec.bit_offset);
                    } else {
                        buf.cells[0] = max_value;
                    }
                    buf.with_mid_addr(|obj| {
                        // SAFETY: `obj` has full-array provenance over `buf.cells[0..3]`; `buf.cells[0]`
                        // is initialized, aligned to `$type`, and unaliased during `with_mid_addr`.
                        assert_eq!(unsafe { spec.load::<$type>(obj, None) }, max_value);
                    });
                }

                #[test]
                fn [<$tname _load_mask>]() {
                    // The test only runs for metadata no smaller than 1 byte
                    if $num_of_bits < 8 {
                        return;
                    }

                    let mut buf = TestHeaderObject::<$type>::new();
                    let spec = HeaderMetadataSpec { bit_offset: 0, num_of_bits: $num_of_bits };
                    buf.with_mid_addr(|obj| {
                        // SAFETY: `obj` points to `buf.cells[1]`, which is zero-initialized, aligned
                        // to `$type`, and unaliased during `with_mid_addr`.
                        assert_eq!(unsafe { spec.load::<$type>(obj, None) }, 0);
                    });
                    let max_value = max_value($num_of_bits) as $type;
                    buf.cells[1] = max_value;
                    buf.with_mid_addr(|obj| {
                        // SAFETY: `obj` points to `buf.cells[1]`, initialized to `max_value`, aligned
                        // to `$type`, and unaliased during `with_mid_addr`.
                        assert_eq!(unsafe { spec.load::<$type>(obj, None) }, max_value);
                        // SAFETY: Same valid, initialized, unaliased `buf.cells[1]` cell as above.
                        assert_eq!(unsafe { spec.load::<$type>(obj, Some(0)) }, 0);
                        // SAFETY: Same valid, initialized, unaliased `buf.cells[1]` cell as above.
                        assert_eq!(unsafe { spec.load::<$type>(obj, Some(0b101)) }, 0b101);
                    });
                }

                #[test]
                fn [<$tname _store>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    let spec = HeaderMetadataSpec { bit_offset: 0, num_of_bits: $num_of_bits };
                    let max_value = max_value($num_of_bits) as $type;
                    buf.with_mid_addr(|obj| {
                        // SAFETY: `obj` points to `buf.cells[1]`, zero-initialized, aligned to `$type`,
                        // and exclusively accessed in this single-threaded test.
                        assert_eq!(unsafe { spec.load::<$type>(obj, None) }, 0);
                        // SAFETY: `obj` points to `buf.cells[1]`, writable, initialized, aligned to
                        // `$type`, and exclusively accessed without aliasing Rust references.
                        unsafe { spec.store::<$type>(obj, max_value, None) };
                        // SAFETY: `buf.cells[1]` remains valid, initialized, and exclusively accessed.
                        assert_eq!(unsafe { spec.load::<$type>(obj, None) }, max_value);
                    });
                    assert_eq!(buf.cells[1], max_value);
                }

                #[test]
                fn [<$tname _store_atomic>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    let spec = HeaderMetadataSpec { bit_offset: 0, num_of_bits: $num_of_bits };
                    let max_value = max_value($num_of_bits) as $type;
                    buf.with_mid_addr(|obj| {
                        assert_eq!(spec.load_atomic::<$type>(obj, None, Ordering::SeqCst), 0);
                        spec.store_atomic::<$type>(obj, max_value, None, Ordering::SeqCst);
                        assert_eq!(spec.load_atomic::<$type>(obj, None, Ordering::SeqCst), max_value);
                    });
                    assert_eq!(buf.cells[1], max_value);
                }

                #[test]
                fn [<$tname _store_next>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    let spec = HeaderMetadataSpec { bit_offset: $num_of_bits, num_of_bits: $num_of_bits };
                    let max_value = max_value($num_of_bits) as $type;
                    buf.with_mid_addr(|obj| {
                        // SAFETY: `obj` has full provenance over `buf.cells[0..3]`; target cell
                        // (`cells[1]` or `cells[2]`) is initialized, aligned, and unaliased.
                        assert_eq!(unsafe { spec.load::<$type>(obj, None) }, 0);
                        // SAFETY: Target cell in `buf.cells[1..=2]` is writable, initialized, aligned
                        // to `$type`, and exclusively accessed during `with_mid_addr`.
                        unsafe { spec.store::<$type>(obj, max_value, None) };
                        // SAFETY: Target cell remains initialized, aligned, and exclusively accessed.
                        assert_eq!(unsafe { spec.load::<$type>(obj, None) }, max_value);
                    });
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
                    buf.with_mid_addr(|obj| {
                        // SAFETY: `obj` has full provenance over `buf.cells[0..3]`; negative offset
                        // resolves in-bounds to `buf.cells[0]`, which is initialized, aligned, and unaliased.
                        assert_eq!(unsafe { spec.load::<$type>(obj, None) }, 0);
                        // SAFETY: `buf.cells[0]` is writable, initialized, aligned to `$type`, and
                        // exclusively accessed without aliasing Rust references during `with_mid_addr`.
                        unsafe { spec.store::<$type>(obj, max_value, None) };
                        // SAFETY: `buf.cells[0]` remains initialized, aligned, and exclusively accessed.
                        assert_eq!(unsafe { spec.load::<$type>(obj, None) }, max_value);
                    });
                    if $num_of_bits < 8 {
                        assert_eq!(buf.cells[0], max_value << (BITS_IN_BYTE as isize + spec.bit_offset));
                    } else {
                        assert_eq!(buf.cells[0], max_value);
                    }
                }

                #[test]
                fn [<$tname _store_mask>]() {
                    // The test only runs for metadata no smaller than 1 byte
                    if $num_of_bits < 8 {
                        return;
                    }

                    let mut buf = TestHeaderObject::<$type>::new();
                    let spec = HeaderMetadataSpec { bit_offset: 0, num_of_bits: $num_of_bits };
                    let max_value = max_value($num_of_bits) as $type;
                    buf.with_mid_addr(|obj| {
                        // SAFETY: `obj` points to `buf.cells[1]`, zero-initialized, aligned to `$type`,
                        // and exclusively accessed during `with_mid_addr`.
                        assert_eq!(unsafe { spec.load::<$type>(obj, None) }, 0);

                        // set to max with mask of all 1s
                        // SAFETY: `obj` points to `buf.cells[1]`, writable, initialized, aligned, unaliased.
                        unsafe { spec.store::<$type>(obj, max_value, Some(max_value)) };
                        // SAFETY: `obj` points to `buf.cells[1]`, initialized, aligned, unaliased.
                        assert_eq!(unsafe { spec.load::<$type>(obj, None) }, max_value);

                        // set to 0
                        // SAFETY: `obj` points to `buf.cells[1]`, writable, initialized, aligned, unaliased.
                        unsafe { spec.store::<$type>(obj, 0, None) };

                        // set to max with mask of 1 bit
                        // SAFETY: `obj` points to `buf.cells[1]`, writable, initialized, aligned, unaliased.
                        unsafe { spec.store::<$type>(obj, max_value, Some(0b10)) };
                        // SAFETY: `obj` points to `buf.cells[1]`, initialized, aligned, unaliased.
                        assert_eq!(unsafe { spec.load::<$type>(obj, None) }, 0b10);
                    });
                    assert_eq!(buf.cells[1], 0b10);
                }

                #[test]
                fn [<$tname _compare_exchange_success>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    buf.with_mid_addr(|obj| {
                        let spec = HeaderMetadataSpec { bit_offset: 0, num_of_bits: $num_of_bits };
                        let old_val = spec.load_atomic::<$type>(obj, None, Ordering::SeqCst);
                        assert_eq!(old_val, 0);

                        let max_value = max_value($num_of_bits) as $type;
                        let res = spec.compare_exchange::<$type>(obj, old_val, max_value, None, Ordering::SeqCst, Ordering::SeqCst);
                        assert!(res.is_ok());
                        assert_eq!(res.unwrap(), old_val);
                        assert_eq!(spec.load_atomic::<$type>(obj, None, Ordering::SeqCst), max_value);
                    })
                }

                #[test]
                fn [<$tname _compare_exchange_fail>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    buf.with_mid_addr(|obj| {
                        let spec = HeaderMetadataSpec { bit_offset: 0, num_of_bits: $num_of_bits };
                        let old_val = spec.load_atomic::<$type>(obj, None, Ordering::SeqCst);
                        assert_eq!(old_val, 0);

                        // Change the value
                        spec.store_atomic::<$type>(obj, 1, None, Ordering::SeqCst);

                        let max_value = max_value($num_of_bits) as $type;
                        let res = spec.compare_exchange::<$type>(obj, old_val, max_value, None, Ordering::SeqCst, Ordering::SeqCst);
                        assert!(res.is_err());
                        assert_eq!(res.err().unwrap(), 1);
                        assert_eq!(spec.load_atomic::<$type>(obj, None, Ordering::SeqCst), 1);
                    })
                }

                #[test]
                fn [<$tname _fetch_add>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    buf.with_mid_addr(|obj| {
                        for bit_offset in (0isize..($type::BITS as isize)).step_by($num_of_bits) {
                            let spec = HeaderMetadataSpec { bit_offset, num_of_bits: $num_of_bits };
                            let max_value = max_value($num_of_bits) as $type;

                            let old_val = spec.load_atomic::<$type>(obj, None, Ordering::SeqCst);
                            assert_eq!(old_val, 0);

                            let old_val_from_fetch = spec.fetch_add::<$type>(obj, max_value, Ordering::SeqCst);
                            assert_eq!(old_val, old_val_from_fetch);
                            assert_eq!(spec.load_atomic::<$type>(obj, None, Ordering::SeqCst), max_value);
                        }
                    })
                }

                #[test]
                fn [<$tname _fetch_add_overflow>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    for bit_offset in (0isize..($type::BITS as isize)).step_by($num_of_bits) {
                        let spec = HeaderMetadataSpec { bit_offset, num_of_bits: $num_of_bits };
                        let max_value = max_value($num_of_bits) as $type;

                        buf.with_mid_addr(|obj| {
                            spec.store_atomic::<$type>(obj, max_value, None, Ordering::SeqCst);
                            let old_val = spec.load_atomic::<$type>(obj, None, Ordering::SeqCst);

                            // add 1 will cause overflow
                            let old_val_from_fetch = spec.fetch_add::<$type>(obj, 1, Ordering::SeqCst);
                            assert_eq!(old_val, old_val_from_fetch);
                            assert_eq!(spec.load_atomic::<$type>(obj, None, Ordering::SeqCst), 0);
                        });
                        assert_eq!(buf.cells[1], 0); // we should not accidentally affect other bits
                    }
                }

                #[test]
                fn [<$tname _fetch_sub>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    buf.with_mid_addr(|obj| {
                        for bit_offset in (0isize..($type::BITS as isize)).step_by($num_of_bits) {
                            let spec = HeaderMetadataSpec { bit_offset, num_of_bits: $num_of_bits };

                            spec.store_atomic::<$type>(obj, 1, None, Ordering::SeqCst);
                            let old_val = spec.load_atomic::<$type>(obj, None, Ordering::SeqCst);
                            assert_eq!(old_val, 1);

                            let old_val_from_fetch = spec.fetch_sub::<$type>(obj, 1, Ordering::SeqCst);
                            assert_eq!(old_val, old_val_from_fetch);
                            assert_eq!(spec.load_atomic::<$type>(obj, None, Ordering::SeqCst), 0);
                        }
                    })
                }

                #[test]
                fn [<$tname _fetch_sub_overflow>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    buf.with_mid_addr(|obj| {
                        for bit_offset in (0isize..($type::BITS as isize)).step_by($num_of_bits) {
                            let spec = HeaderMetadataSpec { bit_offset, num_of_bits: $num_of_bits };
                            let max_value = max_value($num_of_bits) as $type;

                            let old_val = spec.load_atomic::<$type>(obj, None, Ordering::SeqCst);
                            assert_eq!(old_val, 0);

                            let old_val_from_fetch = spec.fetch_sub::<$type>(obj, 1, Ordering::SeqCst);
                            assert_eq!(old_val, old_val_from_fetch);
                            assert_eq!(spec.load_atomic::<$type>(obj, None, Ordering::SeqCst), max_value);
                        }
                    })
                }

                #[test]
                fn [<$tname _fetch_and>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    buf.with_mid_addr(|obj| {
                        for bit_offset in (0isize..($type::BITS as isize)).step_by($num_of_bits) {
                            let spec = HeaderMetadataSpec { bit_offset, num_of_bits: $num_of_bits };
                            let max_value = max_value($num_of_bits) as $type;

                            let old_val = spec.load_atomic::<$type>(obj, None, Ordering::SeqCst);
                            assert_eq!(old_val, 0);

                            let old_val_from_fetch = spec.fetch_and::<$type>(obj, max_value, Ordering::SeqCst);
                            assert_eq!(old_val, old_val_from_fetch);
                            assert_eq!(spec.load_atomic::<$type>(obj, None, Ordering::SeqCst), 0);
                        }
                    })
                }

                #[test]
                fn [<$tname _fetch_or>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    buf.with_mid_addr(|obj| {
                        for bit_offset in (0isize..($type::BITS as isize)).step_by($num_of_bits) {
                            let spec = HeaderMetadataSpec { bit_offset, num_of_bits: $num_of_bits };
                            let max_value = max_value($num_of_bits) as $type;

                            let old_val = spec.load_atomic::<$type>(obj, None, Ordering::SeqCst);
                            assert_eq!(old_val, 0);

                            let old_val_from_fetch = spec.fetch_or::<$type>(obj, max_value, Ordering::SeqCst);
                            assert_eq!(old_val, old_val_from_fetch);
                            assert_eq!(spec.load_atomic::<$type>(obj, None, Ordering::SeqCst), max_value);
                        }
                    })
                }

                #[test]
                fn [<$tname _fetch_update_success>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    buf.with_mid_addr(|obj| {
                        for bit_offset in (0isize..($type::BITS as isize)).step_by($num_of_bits) {
                            let spec = HeaderMetadataSpec { bit_offset, num_of_bits: $num_of_bits };
                            let max_value = max_value($num_of_bits) as $type;

                            let old_val = spec.load_atomic::<$type>(obj, None, Ordering::SeqCst);
                            assert_eq!(old_val, 0);

                            let update_res = spec.fetch_update(obj, Ordering::SeqCst, Ordering::SeqCst, |_x: $type| Some(max_value));
                            assert!(update_res.is_ok());
                            assert_eq!(old_val, update_res.unwrap());
                            assert_eq!(spec.load_atomic::<$type>(obj, None, Ordering::SeqCst), max_value);
                        }
                    })
                }

                #[test]
                fn [<$tname _fetch_update_fail>]() {
                    let mut buf = TestHeaderObject::<$type>::new();
                    buf.with_mid_addr(|obj| {
                        for bit_offset in (0isize..($type::BITS as isize)).step_by($num_of_bits) {
                            let spec = HeaderMetadataSpec { bit_offset, num_of_bits: $num_of_bits };

                            let old_val = spec.load_atomic::<$type>(obj, None, Ordering::SeqCst);
                            assert_eq!(old_val, 0);

                            let update_res = spec.fetch_update(obj, Ordering::SeqCst, Ordering::SeqCst, |_x: $type| None);
                            assert!(update_res.is_err());
                            assert_eq!(old_val, update_res.err().unwrap());
                            assert_eq!(spec.load_atomic::<$type>(obj, None, Ordering::SeqCst), 0);
                        }
                    })
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
}
