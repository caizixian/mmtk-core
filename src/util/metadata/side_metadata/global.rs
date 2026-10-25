#![deny(unsafe_op_in_unsafe_fn)]

use super::*;
use crate::util::constants::{BYTES_IN_PAGE, LOG_BITS_IN_BYTE};
use crate::util::conversions::raw_align_up;
use crate::util::heap::layout::vm_layout::BYTES_IN_CHUNK;
use crate::util::metadata::metadata_val_traits::*;
use crate::util::metadata::side_metadata::layout::*;
#[cfg(feature = "vo_bit")]
use crate::util::metadata::vo_bit::VO_BIT_SIDE_METADATA_SPEC;
use crate::util::os::*;
use crate::util::Address;
use crate::MMAPPER;
use num_traits::FromPrimitive;
use ranges::BitByteRange;
use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};

enum ResolvedSideAtomic<'a, T: MetadataValue> {
    SubByte {
        atomic: &'a AtomicU8,
        lshift: u8,
        mask: u8,
    },
    FullWord {
        atomic: &'a T::Atomic,
    },
}

/// This struct stores the specification of a side metadata bit-set.
/// It is used as an input to the (inline) functions provided by the side metadata module.
///
/// Each plan or policy which uses a metadata bit-set, needs to create an instance of this struct.
///
/// For performance reasons, objects of this struct should be constants.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SideMetadataSpec {
    /// The name for this side metadata.
    pub name: &'static str,
    /// Is this side metadata global? Local metadata is used by certain spaces,
    /// while global metadata is used by all the spaces.
    pub is_global: bool,
    /// The offset for this side metadata.
    pub offset: usize,
    /// Number of bits needed per region. E.g. 0 = 1 bit, 1 = 2 bit.
    pub log_num_of_bits: usize,
    /// Number of bytes of the region. E.g. 3 = 8 bytes, 12 = 4096 bytes.
    pub log_bytes_in_region: usize,
}

impl SideMetadataSpec {
    /// Is this spec using contiguous side metadata? If not, it uses chunked side metadata.
    pub const fn uses_contiguous_side_metadata(&self) -> bool {
        self.is_global || cfg!(target_pointer_width = "64")
    }

    /// Is this spec using chunked side metadata? If not, it uses contiguous side metadata.
    pub const fn uses_chunked_side_metadata(&self) -> bool {
        !self.uses_contiguous_side_metadata()
    }

    /// Get the starting address for a spec of contiguous side metadata.
    pub fn get_starting_address(&self) -> Address {
        debug_assert!(self.uses_contiguous_side_metadata());
        let base = global_side_metadata_base_address();
        base + self.offset
    }

    /// Get the relative offset for a spec of chunked side metadata.
    pub const fn get_offset_for_chunked(&self) -> usize {
        debug_assert!(self.uses_chunked_side_metadata());
        self.offset
    }

    /// Return the upperbound offset for the side metadata. The next side metadata should be laid out at this offset.
    #[cfg(target_pointer_width = "64")]
    pub const fn upper_bound_offset(&self) -> usize {
        debug_assert!(self.uses_contiguous_side_metadata());
        self.offset + metadata_address_range_size(self)
    }

    /// Return the upperbound offset for the side metadata. The next side metadata should be laid out at this offset.
    #[cfg(target_pointer_width = "32")]
    pub const fn upper_bound_offset(&self) -> usize {
        if self.uses_contiguous_side_metadata() {
            self.offset + metadata_address_range_size(self)
        } else {
            self.offset + metadata_bytes_per_chunk(self.log_bytes_in_region, self.log_num_of_bits)
        }
    }

    /// The upper bound address for metadata address computed for this global spec. The computed metadata address
    /// should never be larger than this address. Otherwise, we are accessing the metadata that is laid out
    /// after this spec. This spec must be a contiguous side metadata spec (which uses address
    /// as offset).
    pub fn upper_bound_address_for_contiguous(&self) -> Address {
        debug_assert!(self.uses_contiguous_side_metadata());
        self.get_starting_address() + metadata_address_range_size(self)
    }

    /// The upper bound address for metadata address computed for this global spec. The computed metadata address
    /// should never be larger than this address. Otherwise, we are accessing the metadata that is laid out
    /// after this spec. This spec must be a chunked side metadata spec (which uses relative offset). Only 32 bit local
    /// side metadata uses chunked metadata.
    #[cfg(target_pointer_width = "32")]
    pub fn upper_bound_address_for_chunked(&self, data_addr: Address) -> Address {
        debug_assert!(self.uses_chunked_side_metadata());
        address_to_meta_chunk_addr(data_addr) + self.upper_bound_offset()
    }

    /// Used only for debugging.
    /// This panics if the required metadata is not mapped
    #[cfg(debug_assertions)]
    pub(crate) fn assert_metadata_mapped(&self, data_addr: Address) {
        let meta_start = address_to_meta_address(self, data_addr).align_down(BYTES_IN_PAGE);

        trace!(
            "ensure_metadata_is_mapped({}).meta_start({})",
            data_addr,
            meta_start
        );

        OS::panic_if_unmapped(meta_start, BYTES_IN_PAGE);
    }

    #[cfg(debug_assertions)]
    pub(crate) fn are_different_metadata_bits(&self, addr1: Address, addr2: Address) -> bool {
        let a1 = address_to_meta_address(self, addr1);
        let a2 = address_to_meta_address(self, addr2);
        let s1 = meta_byte_lshift(self, addr1);
        let s2 = meta_byte_lshift(self, addr2);
        (a1, s1) != (a2, s2)
    }

    /// Unconditionally validate `self`'s region shift, bit-width, and byte offset alignment for `T`.
    ///
    /// Because real `SideMetadataSpec` definitions are `const`, LLVM constant-folds these checks to
    /// zero instructions in `--release` while preventing misaligned-atomic or shift-overflow UB on
    /// dynamically constructed `SideMetadataSpec` literals (`pub` fields).
    #[inline(always)]
    pub(crate) const fn assert_spec<T: MetadataValue>(&self) {
        assert!(
            self.log_bytes_in_region < usize::BITS as usize,
            "SideMetadataSpec log_bytes_in_region must be < usize::BITS"
        );
        match self.log_num_of_bits {
            0..=2 => {
                assert!(
                    T::LOG2 == 3,
                    "Sub-byte SideMetadataSpec requires u8 (T::LOG2 == 3)"
                );
            }
            3..=6 => {
                assert!(
                    T::LOG2 == self.log_num_of_bits as u32,
                    "SideMetadataSpec log_num_of_bits must equal T::LOG2"
                );
                let req_align = max_metadata_align::<T>();
                assert!(
                    self.offset % req_align == 0,
                    "SideMetadataSpec offset must be aligned to max_metadata_align::<T>()"
                );
            }
            _ => panic!("SideMetadataSpec > 64-bits is not supported"),
        }
    }

    /// Assert that `T` and `self.offset` alignment match `self`'s bit-width unconditionally (zero
    /// instructions in release builds when `self` is `const`), and check runtime `val` range in
    /// debug builds.
    #[inline(always)]
    #[allow(unused_variables)]
    fn assert_value_type<T: MetadataValue>(&self, val: Option<T>) {
        self.assert_spec::<T>();
        #[cfg(debug_assertions)]
        if self.log_num_of_bits < 3 {
            if let Some(v) = val {
                assert!(
                    v.to_u8().unwrap() < (1 << (1 << self.log_num_of_bits)),
                    "Input value {:?} is invalid for the spec {:?}",
                    v,
                    self
                );
            }
        }
    }

    /// Validate `self` for `T` and project the underlying atomic side-metadata cell for `data_addr`.
    ///
    /// # Safety
    /// * `address_to_meta_address(self, data_addr)` must point to mapped, initialized,
    ///   readable/writable side-metadata memory valid for `'a`.
    /// * No concurrent non-atomic accesses or live non-atomic Rust references (`&` or `&mut`) may
    ///   alias that cell during `'a`, and all concurrent accesses must use atomic operations of the
    ///   same width (`AtomicU8` when `self.log_num_of_bits < 3`, or `T::Atomic` otherwise).
    #[inline(always)]
    unsafe fn resolve_atomic<'a, T: MetadataValue>(
        &self,
        data_addr: Address,
    ) -> ResolvedSideAtomic<'a, T> {
        self.assert_spec::<T>();
        let meta_addr = address_to_meta_address(self, data_addr);
        if self.log_num_of_bits < 3 {
            let lshift = meta_byte_lshift(self, data_addr);
            let mask = meta_byte_mask(self) << lshift;
            // SAFETY: Caller guarantees `meta_addr` is mapped, initialized, and accessed solely via
            // `AtomicU8` for `'a` (`u8::atomic_ref` unconditionally checks `!meta_addr.is_zero()`).
            let atomic = unsafe { <u8 as MetadataValue>::atomic_ref(meta_addr) };
            ResolvedSideAtomic::SubByte {
                atomic,
                lshift,
                mask,
            }
        } else {
            // SAFETY: `assert_spec::<T>()` verified `T::LOG2 == self.log_num_of_bits`,
            // `self.log_bytes_in_region < usize::BITS`, and `self.offset % max_metadata_align::<T>() == 0`;
            // `T::atomic_ref` unconditionally checks `!meta_addr.is_zero()` and
            // `meta_addr.is_aligned_to(T::ATOMIC_ALIGN)`; caller guarantees mapped, initialized
            // side-metadata memory accessed solely via `T::Atomic` for `'a`.
            let atomic = unsafe { T::atomic_ref(meta_addr) };
            ResolvedSideAtomic::FullWord { atomic }
        }
    }

    /// Validate that `[data_addr, data_addr + data_bytes)` maps to a contiguous, byte-aligned
    /// side-metadata range of `meta_bytes` bytes (within a single chunk if chunked), returning
    /// `(meta_addr, meta_bytes)`.
    #[inline]
    fn checked_mapped_byte_range(&self, data_addr: Address, data_bytes: usize) -> (Address, usize) {
        assert!(
            self.log_num_of_bits <= 6 && self.log_bytes_in_region < usize::BITS as usize,
            "Invalid SideMetadataSpec geometry: {:?}",
            self
        );
        let _end = data_addr
            .as_usize()
            .checked_add(data_bytes)
            .expect("SideMetadataSpec slice data range overflow");
        let region_bytes = 1usize << self.log_bytes_in_region;
        assert!(
            data_addr.is_aligned_to(region_bytes) && (data_bytes & (region_bytes - 1)) == 0,
            "data_addr ({}) and data_bytes (0x{:x}) must be aligned to region_bytes (0x{:x})",
            data_addr,
            data_bytes,
            region_bytes
        );
        assert_eq!(
            meta_byte_lshift(self, data_addr),
            0,
            "data_addr ({}) does not start on a side-metadata byte boundary for {:?}",
            data_addr,
            self
        );

        let regions = data_bytes >> self.log_bytes_in_region;
        let meta_bytes = if self.log_num_of_bits >= LOG_BITS_IN_BYTE as usize {
            let bytes_per_region_shift = self.log_num_of_bits - LOG_BITS_IN_BYTE as usize;
            regions
                .checked_mul(1usize << bytes_per_region_shift)
                .expect("SideMetadataSpec slice byte length overflow")
        } else {
            let regions_per_byte_shift = LOG_BITS_IN_BYTE as usize - self.log_num_of_bits;
            let regions_per_byte = 1usize << regions_per_byte_shift;
            assert_eq!(
                regions & (regions_per_byte - 1),
                0,
                "data_bytes (0x{:x}) does not cover an integral number of metadata bytes for {:?}",
                data_bytes,
                self
            );
            regions >> regions_per_byte_shift
        };

        if data_bytes > 0 && self.uses_chunked_side_metadata() {
            let last_byte = data_addr + (data_bytes - 1);
            assert_eq!(
                data_addr.align_down(BYTES_IN_CHUNK),
                last_byte.align_down(BYTES_IN_CHUNK),
                "Chunked side-metadata slice for {:?} cannot span multiple chunks",
                self
            );
        }

        let meta_addr = address_to_meta_address(self, data_addr);
        assert!(
            !meta_addr.is_zero(),
            "SideMetadataSpec slice resolved to null metadata address"
        );
        assert!(
            meta_bytes <= isize::MAX as usize,
            "SideMetadataSpec slice byte length (0x{:x}) exceeds isize::MAX",
            meta_bytes
        );
        assert!(
            meta_addr.as_usize().checked_add(meta_bytes).is_some(),
            "SideMetadataSpec slice metadata address range overflow ({} + 0x{:x})",
            meta_addr,
            meta_bytes
        );
        #[cfg(all(debug_assertions, feature = "extreme_assertions"))]
        if meta_bytes > 0 {
            self.assert_metadata_mapped(data_addr);
        }
        (meta_addr, meta_bytes)
    }

    /// Validate `self`'s `<= 8`-bit geometry and compute the exact data byte count covered by
    /// `meta_bytes` contiguous side-metadata bytes without shift or multiply wraparound.
    #[inline(always)]
    fn checked_meta_to_data_bytes(&self, meta_bytes: usize) -> usize {
        assert!(
            self.log_num_of_bits <= LOG_BITS_IN_BYTE as usize
                && self.log_bytes_in_region < usize::BITS as usize,
            "Invalid <= 8-bit SideMetadataSpec geometry: {:?}",
            self
        );
        let shift = self.log_bytes_in_region + (LOG_BITS_IN_BYTE as usize - self.log_num_of_bits);
        assert!(
            shift < usize::BITS as usize,
            "SideMetadataSpec data-to-metadata ratio shift ({}) overflows usize: {:?}",
            shift,
            self
        );
        meta_bytes
            .checked_mul(1usize << shift)
            .expect("SideMetadataSpec slice data_bytes overflow")
    }

    /// Project the contiguous `N`-byte side-metadata range starting at `data_addr` as
    /// `&'a [AtomicU8; N]`.
    ///
    /// # Safety
    /// * `self.log_num_of_bits <= 3`, `N <= isize::MAX as usize`, `data_addr` must fall on a byte
    ///   boundary in `self`'s side metadata without data or metadata range overflow, and if
    ///   `N > 0 && self.uses_chunked_side_metadata()`, the covered data range must lie within a
    ///   single `BYTES_IN_CHUNK` chunk.
    /// * The `N` side-metadata bytes starting at `address_to_meta_address(self, data_addr)` must be
    ///   mapped, initialized, and remain mapped for `'a`.
    /// * For the entire lifetime `'a`, no thread may perform non-atomic reads/writes or mixed-width
    ///   atomic accesses (wider than `AtomicU8`), and no live non-atomic Rust reference (`&[u8]` or
    ///   `&mut [u8]`) may alias `[meta_addr, meta_addr + N)`.
    #[inline(always)]
    pub(crate) unsafe fn mapped_atomic_bytes_unchecked<'a, const N: usize>(
        &self,
        data_addr: Address,
    ) -> &'a [AtomicU8; N] {
        let data_bytes = self.checked_meta_to_data_bytes(N);
        let (meta_addr, meta_bytes) = self.checked_mapped_byte_range(data_addr, data_bytes);
        assert_eq!(meta_bytes, N);
        // SAFETY: `checked_meta_to_data_bytes` and `checked_mapped_byte_range` verified byte
        // alignment, single-chunk containment, `!meta_addr.is_zero()`, `N <= isize::MAX as usize`,
        // and non-wrapping `meta_addr + N` (`AtomicU8` has alignment 1); the caller guarantees
        // `[meta_addr, meta_addr + N)` is mapped, initialized, valid for `'a`, and accessed solely
        // via `AtomicU8`.
        unsafe { &*meta_addr.to_ptr::<[AtomicU8; N]>() }
    }

    /// Project the contiguous `ROWS * COLS`-byte side-metadata range starting at `data_addr` as a
    /// 2D array `&'a [[AtomicU8; COLS]; ROWS]` (e.g. Immix `MappedBlock::rc_table`
    /// `&[[AtomicU8; BYTES_PER_LINE]; Block::LINES]` on Rust 1.84 without `<[T]>::as_chunks`).
    ///
    /// # Safety
    /// Same preconditions as [`Self::mapped_atomic_bytes_unchecked`] for `N = ROWS * COLS`
    /// contiguous side-metadata bytes.
    #[allow(dead_code)] // Consumed by Wave 2B (`immix` `MappedBlock::rc_table`).
    #[inline(always)]
    pub(crate) unsafe fn mapped_atomic_byte_chunks_unchecked<
        'a,
        const ROWS: usize,
        const COLS: usize,
    >(
        &self,
        data_addr: Address,
    ) -> &'a [[AtomicU8; COLS]; ROWS] {
        let total_meta_bytes = ROWS
            .checked_mul(COLS)
            .expect("mapped_atomic_byte_chunks_unchecked ROWS * COLS overflow");
        let data_bytes = self.checked_meta_to_data_bytes(total_meta_bytes);
        let (meta_addr, meta_bytes) = self.checked_mapped_byte_range(data_addr, data_bytes);
        assert_eq!(meta_bytes, total_meta_bytes);
        // SAFETY: `checked_meta_to_data_bytes` and `checked_mapped_byte_range` verified byte
        // alignment, single-chunk containment, `!meta_addr.is_zero()`,
        // `total_meta_bytes <= isize::MAX as usize`, and non-wrapping
        // `meta_addr + total_meta_bytes` (`[[AtomicU8; COLS]; ROWS]` has alignment 1 and size
        // `ROWS * COLS`); the caller guarantees the range is mapped, initialized, valid for `'a`,
        // and accessed solely via `AtomicU8`.
        unsafe { &*meta_addr.to_ptr::<[[AtomicU8; COLS]; ROWS]>() }
    }

    /// Project the contiguous side-metadata bytes covering `[data_addr, data_addr + data_bytes)` as
    /// a shared byte slice `&'a [u8]`.
    ///
    /// # Safety
    /// * `data_addr` and `data_addr + data_bytes` must both fall on exact byte boundaries in
    ///   `self`'s side metadata without data or metadata range overflow (`meta_bytes <= isize::MAX as usize`),
    ///   and if `data_bytes > 0 && self.uses_chunked_side_metadata()`, `[data_addr, data_addr + data_bytes)`
    ///   must lie within a single `BYTES_IN_CHUNK` chunk.
    /// * The `meta_bytes` bytes starting at `address_to_meta_address(self, data_addr)` must be
    ///   mapped, initialized, and remain mapped for `'a`.
    /// * For the entire lifetime `'a`, no thread may mutate (atomically or non-atomically) any byte
    ///   in `[meta_addr, meta_addr + meta_bytes)`, and no `&mut` reference may alias that range.
    #[allow(dead_code)] // Consumed by Wave 2B (`malloc_ms` `MallocExclusiveChunk`).
    #[inline]
    pub(crate) unsafe fn mapped_bytes_unchecked<'a>(
        &self,
        data_addr: Address,
        data_bytes: usize,
    ) -> &'a [u8] {
        let (meta_addr, meta_bytes) = self.checked_mapped_byte_range(data_addr, data_bytes);
        // SAFETY: `checked_mapped_byte_range` verified non-null `meta_addr` (`u8` has alignment 1),
        // `meta_bytes <= isize::MAX as usize`, non-wrapping `meta_addr + meta_bytes`, and
        // single-chunk containment; the caller guarantees `[meta_addr, meta_addr + meta_bytes)` is
        // mapped, initialized, and immutable without `&mut` or atomic-write aliases for `'a`.
        unsafe { std::slice::from_raw_parts(meta_addr.to_ptr::<u8>(), meta_bytes) }
    }

    /// Project the contiguous side-metadata bytes covering `[data_addr, data_addr + data_bytes)` as
    /// an exclusive mutable byte slice `&'a mut [u8]`.
    ///
    /// # Safety
    /// * `data_addr` and `data_addr + data_bytes` must both fall on exact byte boundaries in
    ///   `self`'s side metadata without data or metadata range overflow (`meta_bytes <= isize::MAX as usize`),
    ///   and if `data_bytes > 0 && self.uses_chunked_side_metadata()`, `[data_addr, data_addr + data_bytes)`
    ///   must lie within a single `BYTES_IN_CHUNK` chunk.
    /// * The `meta_bytes` bytes starting at `address_to_meta_address(self, data_addr)` must be
    ///   mapped, writable, initialized, and remain mapped for `'a`.
    /// * Exclusive aliasing invariant: for the entire lifetime `'a`, the caller must have unique
    ///   access to `[meta_addr, meta_addr + meta_bytes)` — no other `&[u8]`, `&mut [u8]`,
    ///   `&Atomic*`, or raw-pointer/`SideMetadataSpec` read/write/bzero may alias or access any
    ///   byte in this range.
    #[allow(dead_code)] // Consumed by Wave 2B (`malloc_ms` `MallocExclusiveChunk`).
    #[inline]
    pub(crate) unsafe fn mapped_bytes_mut_unchecked<'a>(
        &self,
        data_addr: Address,
        data_bytes: usize,
    ) -> &'a mut [u8] {
        let (meta_addr, meta_bytes) = self.checked_mapped_byte_range(data_addr, data_bytes);
        // SAFETY: `checked_mapped_byte_range` verified non-null `meta_addr` (`u8` has alignment 1),
        // `meta_bytes <= isize::MAX as usize`, non-wrapping `meta_addr + meta_bytes`, and
        // single-chunk containment; the caller guarantees `[meta_addr, meta_addr + meta_bytes)` is
        // mapped, initialized, writable, and uniquely borrowed without any aliasing references or
        // concurrent/interleaved raw-pointer accesses for `'a`.
        unsafe { std::slice::from_raw_parts_mut(meta_addr.to_mut_ptr::<u8>(), meta_bytes) }
    }

    /// Check with the mmapper to see if side metadata is mapped for the spec for the data address.
    pub(crate) fn is_mapped(&self, data_addr: Address) -> bool {
        use crate::MMAPPER;
        let meta_addr = address_to_meta_address(self, data_addr);
        MMAPPER.is_mapped_address(meta_addr)
    }

    /// This method is used for bulk zeroing side metadata for a data address range.
    pub(crate) fn zero_meta_bits(
        meta_start_addr: Address,
        meta_start_bit: u8,
        meta_end_addr: Address,
        meta_end_bit: u8,
    ) {
        let mut visitor = |range| {
            match range {
                BitByteRange::Bytes { start, end } => {
                    crate::util::memory::zero(start, end - start);
                    false
                }
                BitByteRange::BitsInByte {
                    addr,
                    bit_start,
                    bit_end,
                } => {
                    // we are zeroing selected bit in one byte
                    // Get a mask that the bits we need to zero are set to zero, and the other bits are 1.
                    let mask: u8 =
                        u8::MAX.checked_shl(bit_end as u32).unwrap_or(0) | !(u8::MAX << bit_start);
                    // SAFETY: `zero_meta_bits` (preserved with its cross-slice `pub(crate)`
                    // signature for `test_private`) requires the side-metadata bit range
                    // `[meta_start_addr:meta_start_bit, meta_end_addr:meta_end_bit)` to be non-null,
                    // mapped, and initialized (`AtomicU8` has alignment 1), with all concurrent
                    // accesses to shared boundary bytes using atomic byte operations without live
                    // `&mut` references.
                    unsafe { <u8 as MetadataValue>::fetch_and(addr, mask, Ordering::SeqCst) };
                    false
                }
            }
        };
        ranges::break_bit_range(
            meta_start_addr,
            meta_start_bit,
            meta_end_addr,
            meta_end_bit,
            true,
            &mut visitor,
        );
    }

    /// This method is used for bulk setting side metadata for a data address range.
    pub(crate) fn set_meta_bits(
        meta_start_addr: Address,
        meta_start_bit: u8,
        meta_end_addr: Address,
        meta_end_bit: u8,
    ) {
        let mut visitor = |range| {
            match range {
                BitByteRange::Bytes { start, end } => {
                    crate::util::memory::set(start, 0xff, end - start);
                    false
                }
                BitByteRange::BitsInByte {
                    addr,
                    bit_start,
                    bit_end,
                } => {
                    // we are setting selected bits in one byte
                    // Get a mask that the bits we need to set are 1, and the other bits are 0.
                    let mask: u8 = !(u8::MAX.checked_shl(bit_end as u32).unwrap_or(0))
                        & (u8::MAX << bit_start);
                    // SAFETY: `set_meta_bits` (preserved with its cross-slice `pub(crate)`
                    // signature for `test_private`) requires the side-metadata bit range
                    // `[meta_start_addr:meta_start_bit, meta_end_addr:meta_end_bit)` to be non-null,
                    // mapped, and initialized (`AtomicU8` has alignment 1), with all concurrent
                    // accesses to shared boundary bytes using atomic byte operations without live
                    // `&mut` references.
                    unsafe { <u8 as MetadataValue>::fetch_or(addr, mask, Ordering::SeqCst) };
                    false
                }
            }
        };
        ranges::break_bit_range(
            meta_start_addr,
            meta_start_bit,
            meta_end_addr,
            meta_end_bit,
            true,
            &mut visitor,
        );
    }

    /// This method does bulk update for the given data range. It calculates the metadata bits for the given data range,
    /// and invoke the given method to update the metadata bits.
    pub(super) fn bulk_update_metadata(
        &self,
        start: Address,
        size: usize,
        update_meta_bits: &impl Fn(Address, u8, Address, u8),
    ) {
        // Update bits for a contiguous side metadata spec. We can simply calculate the data end address, and
        // calculate the metadata address for the data end.
        let update_contiguous = |data_start: Address, data_bytes: usize| {
            if data_bytes == 0 {
                return;
            }
            let meta_start = address_to_meta_address(self, data_start);
            let meta_start_shift = meta_byte_lshift(self, data_start);
            let meta_end = address_to_meta_address(self, data_start + data_bytes);
            let meta_end_shift = meta_byte_lshift(self, data_start + data_bytes);
            update_meta_bits(meta_start, meta_start_shift, meta_end, meta_end_shift);
        };

        // Update bits for a discontiguous side metadata spec (chunked metadata). The side metadata for different
        // chunks are stored in discontiguous memory. For example, Chunk #2 follows Chunk #1, but the side metadata
        // for Chunk #2 does not immediately follow the side metadata for Chunk #1. So when we bulk update metadata for Chunk #1,
        // we cannot update up to the metadata address for the Chunk #2 start. Otherwise it may modify unrelated metadata
        // between the two chunks' metadata.
        // Instead, we compute how many bytes/bits we need to update.
        // The data for which the metadata will be updates has to be in the same chunk.
        #[cfg(target_pointer_width = "32")]
        let update_discontiguous = |data_start: Address, data_bytes: usize| {
            use crate::util::constants::BITS_IN_BYTE;
            if data_bytes == 0 {
                return;
            }
            debug_assert_eq!(
                data_start.align_down(BYTES_IN_CHUNK),
                (data_start + data_bytes - 1).align_down(BYTES_IN_CHUNK),
                "The data to be zeroed in discontiguous specs needs to be in the same chunk"
            );
            let meta_start = address_to_meta_address(self, data_start);
            let meta_start_shift = meta_byte_lshift(self, data_start);
            // How many bits we need to zero for data_bytes
            let meta_total_bits = (data_bytes >> self.log_bytes_in_region) << self.log_num_of_bits;
            let meta_delta_bytes = meta_total_bits >> LOG_BITS_IN_BYTE;
            let meta_delta_bits: u8 = (meta_total_bits % BITS_IN_BYTE) as u8;
            // Calculate the end byte/addr and end bit
            let (meta_end, meta_end_shift) = {
                let mut end_addr = meta_start + meta_delta_bytes;
                let mut end_bit = meta_start_shift + meta_delta_bits;
                if end_bit >= BITS_IN_BYTE as u8 {
                    end_bit -= BITS_IN_BYTE as u8;
                    end_addr += 1usize;
                }
                (end_addr, end_bit)
            };

            update_meta_bits(meta_start, meta_start_shift, meta_end, meta_end_shift);
        };

        if cfg!(target_pointer_width = "64") || self.is_global {
            update_contiguous(start, size);
        }
        #[cfg(target_pointer_width = "32")]
        if !self.is_global {
            // per chunk policy-specific metadata for 32-bits targets
            let chunk_num = ((start + size).align_down(BYTES_IN_CHUNK)
                - start.align_down(BYTES_IN_CHUNK))
                / BYTES_IN_CHUNK;
            if chunk_num == 0 {
                update_discontiguous(start, size);
            } else {
                let second_data_chunk = start.align_up(BYTES_IN_CHUNK);
                // bzero the first sub-chunk
                update_discontiguous(start, second_data_chunk - start);

                let last_data_chunk = (start + size).align_down(BYTES_IN_CHUNK);
                // bzero the last sub-chunk
                update_discontiguous(last_data_chunk, start + size - last_data_chunk);
                let mut next_data_chunk = second_data_chunk;

                // bzero all chunks in the middle
                while next_data_chunk != last_data_chunk {
                    update_discontiguous(next_data_chunk, BYTES_IN_CHUNK);
                    next_data_chunk += BYTES_IN_CHUNK;
                }
            }
        }
    }

    /// Bulk-zero a specific metadata for a memory region. Note that this method is more sophisiticated than a simple memset, especially in the following
    /// cases:
    /// * the metadata for the range includes partial bytes (a few bits in the same byte).
    /// * for 32 bits local side metadata, the side metadata is stored in discontiguous chunks, we will have to bulk zero for each chunk's side metadata.
    ///
    /// # Arguments
    ///
    /// * `start`: The starting address of a memory region. The side metadata starting from this data address will be zeroed.
    /// * `size`: The size of the memory region.
    pub fn bzero_metadata(&self, start: Address, size: usize) {
        #[cfg(feature = "extreme_assertions")]
        let _lock = sanity::SANITY_LOCK.lock().unwrap();

        #[cfg(feature = "extreme_assertions")]
        sanity::verify_bzero(self, start, size);

        self.bulk_update_metadata(start, size, &Self::zero_meta_bits)
    }

    /// Bulk set a specific metadata for a memory region. Note that this method is more sophisiticated than a simple memset, especially in the following
    /// cases:
    /// * the metadata for the range includes partial bytes (a few bits in the same byte).
    /// * for 32 bits local side metadata, the side metadata is stored in discontiguous chunks, we will have to bulk set for each chunk's side metadata.
    ///
    /// # Arguments
    ///
    /// * `start`: The starting address of a memory region. The side metadata starting from this data address will be set to all 1s in the bits.
    /// * `size`: The size of the memory region.
    pub fn bset_metadata(&self, start: Address, size: usize) {
        #[cfg(feature = "extreme_assertions")]
        let _lock = sanity::SANITY_LOCK.lock().unwrap();

        #[cfg(feature = "extreme_assertions")]
        sanity::verify_bset(self, start, size);

        self.bulk_update_metadata(start, size, &Self::set_meta_bits)
    }

    /// Bulk copy the `other` side metadata for a memory region to this side metadata.
    ///
    /// Both `self` and `other` must be contiguous side-metadata specs (`uses_contiguous_side_metadata()`)
    /// with identical `log_bytes_in_region` and `log_num_of_bits`, and must either refer to
    /// non-overlapping side-metadata ranges or be the exact same spec (`self == other`).
    ///
    /// # Arguments
    ///
    /// * `start`: The starting address of a memory region.
    /// * `size`: The size of the memory region.
    /// * `other`: The other metadata to copy from.
    pub fn bcopy_metadata_contiguous(&self, start: Address, size: usize, other: &SideMetadataSpec) {
        #[cfg(feature = "extreme_assertions")]
        let _lock = sanity::SANITY_LOCK.lock().unwrap();

        #[cfg(feature = "extreme_assertions")]
        sanity::verify_bcopy(self, start, size, other);

        assert!(
            self.uses_contiguous_side_metadata() && other.uses_contiguous_side_metadata(),
            "bcopy_metadata_contiguous requires contiguous side-metadata specs: dst={:?}, src={:?}",
            self,
            other
        );
        assert_eq!(other.log_bytes_in_region, self.log_bytes_in_region);
        assert_eq!(other.log_num_of_bits, self.log_num_of_bits);

        let dst_meta_start_addr = address_to_meta_address(self, start);
        let dst_meta_start_bit = meta_byte_lshift(self, start);
        let dst_meta_end_addr = address_to_meta_address(self, start + size);
        let dst_meta_end_bit = meta_byte_lshift(self, start + size);

        let src_meta_start_addr = address_to_meta_address(other, start);
        let src_meta_start_bit = meta_byte_lshift(other, start);

        debug_assert_eq!(dst_meta_start_bit, src_meta_start_bit);

        let mut visitor = |range| {
            match range {
                BitByteRange::Bytes {
                    start: dst_start,
                    end: dst_end,
                } => {
                    let byte_offset = dst_start - dst_meta_start_addr;
                    let src_start = src_meta_start_addr + byte_offset;
                    let copy_bytes = dst_end - dst_start;
                    // SAFETY: Both `self` and `other` are verified contiguous specs with matching
                    // `log_bytes_in_region` and `log_num_of_bits`, and `bcopy_metadata_contiguous`
                    // requires both ranges for `[start, start + size)` to be non-null, mapped,
                    // initialized, non-overlapping (or identical `self == other`), and quiescent
                    // across full interior bytes during the bulk copy (`u8` has alignment 1).
                    unsafe {
                        std::ptr::copy::<u8>(
                            src_start.to_ptr(),
                            dst_start.to_mut_ptr(),
                            copy_bytes,
                        );
                    }
                    false
                }
                BitByteRange::BitsInByte {
                    addr: dst,
                    bit_start,
                    bit_end,
                } => {
                    let byte_offset = dst - dst_meta_start_addr;
                    let src = src_meta_start_addr + byte_offset;
                    // Get a mask where the bits to copy are 1 and the remaining bits are 0.
                    let mask: u8 = !(u8::MAX.checked_shl(bit_end as u32).unwrap_or(0))
                        & (u8::MAX << bit_start);
                    // SAFETY: `src` is a non-null, mapped, initialized boundary byte in `other`'s
                    // contiguous side metadata for `[start, start + size)` (`AtomicU8` has alignment
                    // 1), non-overlapping with `dst` (or identical when `self == other`), with no
                    // concurrent non-atomic writes or `&mut` aliases.
                    let old_src =
                        unsafe { <u8 as MetadataValue>::load_atomic(src, Ordering::Relaxed) };
                    // SAFETY: `dst` is a non-null, mapped, initialized boundary byte in `self`'s
                    // contiguous side metadata for `[start, start + size)` (`AtomicU8` has alignment
                    // 1). Using `fetch_update` atomically preserves `!mask` bits if another thread
                    // concurrently copies an adjacent range sharing this boundary byte.
                    let _ = unsafe {
                        <u8 as MetadataValue>::fetch_update(
                            dst,
                            Ordering::Relaxed,
                            Ordering::Relaxed,
                            |old_dst| Some((old_src & mask) | (old_dst & !mask)),
                        )
                    };
                    false
                }
            }
        };

        ranges::break_bit_range(
            dst_meta_start_addr,
            dst_meta_start_bit,
            dst_meta_end_addr,
            dst_meta_end_bit,
            true,
            &mut visitor,
        );
    }

    /// This is a wrapper method for implementing side metadata access.
    /// `assert_value_type` checks spec width and offset alignment unconditionally (constant-folded
    /// to zero instructions for `const` specs in release builds), while debug/extreme-assertion
    /// builds additionally check runtime value bounds, mapping, and the sanity map.
    #[allow(unused_variables)] // data_addr is not used in release build
    fn side_metadata_access<
        const CHECK_VALUE: bool,
        T: MetadataValue,
        R: Copy,
        F: FnOnce() -> R,
        V: FnOnce(R),
    >(
        &self,
        data_addr: Address,
        input: Option<T>,
        access_func: F,
        verify_func: V,
    ) -> R {
        // With extreme assertions, we maintain a sanity table for each side metadata access. For whatever we store in
        // side metadata, we store in the sanity table. So we can use that table to check if its results are conssitent
        // with the actual side metadata.
        // To achieve this, we need to apply a lock when we access side metadata. This will hide some concurrency bugs,
        // but makes it possible for us to assert our side metadata implementation is correct.
        #[cfg(feature = "extreme_assertions")]
        let _lock = sanity::SANITY_LOCK.lock().unwrap();

        if CHECK_VALUE {
            self.assert_value_type::<T>(input);
        }
        #[cfg(all(debug_assertions, feature = "extreme_assertions"))]
        self.assert_metadata_mapped(data_addr);

        // Actual access to the side metadata
        let ret = access_func();

        // Verifying the side metadata: checks the result with the sanity table, or store some results to the sanity table
        if CHECK_VALUE {
            verify_func(ret);
        }

        ret
    }

    /// Non-atomic load of metadata.
    ///
    /// # Safety
    ///
    /// * The side-metadata memory corresponding to `data_addr` under `self` must be mapped,
    ///   initialized, non-null, and properly aligned for `T` (`u8` when `self.log_num_of_bits < 3`,
    ///   or `T::LOG2 == self.log_num_of_bits` otherwise).
    /// * No other thread may concurrently write to or atomically modify the underlying metadata
    ///   byte/word (including adjacent regions sharing the same byte when `self.log_num_of_bits < 3`),
    ///   and no live `&mut` reference may alias that location.
    pub unsafe fn load<T: MetadataValue>(&self, data_addr: Address) -> T {
        self.side_metadata_access::<true, T, _, _, _>(
            data_addr,
            None,
            || {
                let meta_addr = address_to_meta_address(self, data_addr);
                let bits_num_log = self.log_num_of_bits;
                if bits_num_log < 3 {
                    let lshift = meta_byte_lshift(self, data_addr);
                    let mask = meta_byte_mask(self) << lshift;
                    // SAFETY: Caller guarantees `meta_addr` is non-null, mapped, initialized
                    // (`u8` has alignment 1), and free of concurrent writes, atomic modifications,
                    // or live `&mut` aliases.
                    let byte_val = unsafe { meta_addr.load::<u8>() };

                    FromPrimitive::from_u8((byte_val & mask) >> lshift).unwrap()
                } else {
                    // SAFETY: Caller guarantees `meta_addr` is non-null, mapped, initialized,
                    // aligned to `T` (`base + self.offset` is word/chunk-aligned and the region
                    // byte index `(data_addr >> log_bytes_in_region) << (bits_num_log - 3)` is a
                    // multiple of `size_of::<T>()`), and free of concurrent writes, atomic
                    // modifications, or live `&mut` aliases.
                    unsafe { meta_addr.load::<T>() }
                }
            },
            |_v| {
                #[cfg(feature = "extreme_assertions")]
                sanity::verify_load(self, data_addr, _v);
            },
        )
    }

    /// Non-atomic store of metadata.
    ///
    /// # Safety
    ///
    /// * The side-metadata memory corresponding to `data_addr` under `self` must be mapped,
    ///   initialized (for sub-byte read-modify-write), writable, non-null, and properly aligned for
    ///   `T` (`u8` when `self.log_num_of_bits < 3`, or `T::LOG2 == self.log_num_of_bits` otherwise).
    /// * Caller must have exclusive access to the underlying metadata byte/word: no other thread
    ///   may concurrently read, write, or atomically access it (including adjacent regions sharing
    ///   the same byte when `self.log_num_of_bits < 3`), and no live Rust reference may alias it.
    pub unsafe fn store<T: MetadataValue>(&self, data_addr: Address, metadata: T) {
        self.side_metadata_access::<true, T, _, _, _>(
            data_addr,
            Some(metadata),
            || {
                let meta_addr = address_to_meta_address(self, data_addr);
                let bits_num_log = self.log_num_of_bits;
                if bits_num_log < 3 {
                    let lshift = meta_byte_lshift(self, data_addr);
                    let mask = meta_byte_mask(self) << lshift;
                    // SAFETY: Caller guarantees `meta_addr` is non-null, mapped, initialized
                    // (`u8` has alignment 1), and exclusively accessed without concurrent accesses
                    // or aliasing Rust references.
                    let old_val = unsafe { meta_addr.load::<u8>() };
                    let new_val = (old_val & !mask) | (metadata.to_u8().unwrap() << lshift);

                    // SAFETY: Same exclusive, non-null, mapped 1-byte metadata location as above.
                    unsafe {
                        meta_addr.store::<u8>(new_val);
                    }
                } else {
                    // SAFETY: Caller guarantees `meta_addr` is non-null, mapped, writable, aligned
                    // to `T`, and exclusively accessed without concurrent accesses or aliasing
                    // Rust references.
                    unsafe {
                        meta_addr.store::<T>(metadata);
                    }
                }
            },
            |_| {
                #[cfg(feature = "extreme_assertions")]
                sanity::verify_store(self, data_addr, metadata);
            },
        )
    }

    /// Non-atomically load a raw byte from the side metadata byte that is mapped to the data address.
    /// Unlike [`SideMetadataSpec::load`], this always reads a whole byte regardless of the number of
    /// bits used by this spec, and does not mask/shift out unrelated bits sharing that byte.
    ///
    /// # Safety
    ///
    /// * `self`'s side-metadata byte for `data_addr` must be non-null, mapped, readable, and
    ///   initialized.
    /// * No other thread may concurrently write to or atomically modify that metadata byte for the
    ///   duration of this call, and no live `&mut` reference may alias it.
    pub unsafe fn load_byte(&self, data_addr: Address) -> u8 {
        let meta_addr = address_to_meta_address(self, data_addr);
        // SAFETY: Caller guarantees `meta_addr` is non-null, mapped, initialized (`u8` has
        // alignment 1), and free of concurrent writes, atomic modifications, or live `&mut` aliases.
        unsafe { meta_addr.load::<u8>() }
    }

    /// Non-atomically store a raw byte to the side metadata byte that is mapped to the data address.
    ///
    /// # Safety
    ///
    /// * `self`'s side-metadata byte for `data_addr` must be non-null, mapped, and writable.
    /// * No other thread may concurrently read, write, or atomically access that metadata byte
    ///   (note that writing the whole byte overwrites all regions sharing it when
    ///   `self.log_num_of_bits < 3`), and no live Rust reference may alias it.
    pub unsafe fn store_byte_relaxed(&self, data_addr: Address, byte: u8) {
        let meta_addr = address_to_meta_address(self, data_addr);
        // SAFETY: Caller guarantees `meta_addr` is non-null, mapped, writable (`u8` has
        // alignment 1), and exclusively accessed without concurrent accesses or aliasing references.
        unsafe {
            meta_addr.store::<u8>(byte);
        }
    }

    /// Loads a value from the side metadata for the given address.
    /// This method has similar semantics to `load` in Rust atomics.
    pub fn load_atomic<T: MetadataValue>(&self, data_addr: Address, order: Ordering) -> T {
        self.side_metadata_access::<true, T, _, _, _>(
            data_addr,
            None,
            || {
                // SAFETY: By `SideMetadataSpec`'s mapped-region contract (checked under
                // `extreme_assertions`) and unconditional `assert_spec`, `data_addr`'s metadata
                // cell is mapped, initialized, aligned, and accessed atomically without non-atomic
                // write races or `&mut` aliases.
                match unsafe { self.resolve_atomic::<T>(data_addr) } {
                    ResolvedSideAtomic::SubByte {
                        atomic,
                        lshift,
                        mask,
                    } => FromPrimitive::from_u8((atomic.load(order) & mask) >> lshift).unwrap(),
                    ResolvedSideAtomic::FullWord { atomic } => atomic.load_val(order),
                }
            },
            |_v| {
                #[cfg(feature = "extreme_assertions")]
                sanity::verify_load(self, data_addr, _v);
            },
        )
    }

    /// Store the given value to the side metadata for the given address.
    /// This method has similar semantics to `store` in Rust atomics.
    pub fn store_atomic<T: MetadataValue>(&self, data_addr: Address, metadata: T, order: Ordering) {
        self.side_metadata_access::<true, T, _, _, _>(
            data_addr,
            Some(metadata),
            || {
                // SAFETY: By `SideMetadataSpec`'s mapped-region contract and unconditional
                // `assert_spec`, `data_addr`'s metadata cell is mapped, initialized, writable, and
                // accessed atomically without non-atomic races or live non-atomic references.
                match unsafe { self.resolve_atomic::<T>(data_addr) } {
                    ResolvedSideAtomic::SubByte {
                        atomic,
                        lshift,
                        mask,
                    } => {
                        let metadata_u8 = metadata.to_u8().unwrap();
                        let fetch_order = load_order_for_rmw(order);
                        let _ = atomic.fetch_update_val(order, fetch_order, |v: u8| {
                            Some((v & !mask) | (metadata_u8 << lshift))
                        });
                    }
                    ResolvedSideAtomic::FullWord { atomic } => atomic.store_val(metadata, order),
                }
            },
            |_| {
                #[cfg(feature = "extreme_assertions")]
                sanity::verify_store(self, data_addr, metadata);
            },
        )
    }

    /// Non-atomically store zero to the side metadata for the given address.
    /// This method mainly facilitates clearing multiple metadata specs for the same address in a loop.
    ///
    /// # Safety
    ///
    /// * `self`'s side-metadata entry for `data_addr` must be non-null, mapped, initialized,
    ///   writable, and aligned for its width (`self.log_num_of_bits <= 6`).
    /// * No other thread may concurrently read, write, or atomically access the underlying
    ///   metadata byte/word, and no live Rust reference may alias it.
    pub unsafe fn set_zero(&self, data_addr: Address) {
        use num_traits::Zero;
        match self.log_num_of_bits {
            0..=3 => {
                // SAFETY: Caller upholds `SideMetadataSpec::store` exclusive-access and mapped
                // invariants for `data_addr`, and `log_num_of_bits <= 3` uses `u8`.
                unsafe { self.store(data_addr, u8::zero()) }
            }
            4 => {
                // SAFETY: Caller upholds `SideMetadataSpec::store` invariants for `data_addr`,
                // and `log_num_of_bits == 4` uses `u16`.
                unsafe { self.store(data_addr, u16::zero()) }
            }
            5 => {
                // SAFETY: Caller upholds `SideMetadataSpec::store` invariants for `data_addr`,
                // and `log_num_of_bits == 5` uses `u32`.
                unsafe { self.store(data_addr, u32::zero()) }
            }
            6 => {
                // SAFETY: Caller upholds `SideMetadataSpec::store` invariants for `data_addr`,
                // and `log_num_of_bits == 6` uses `u64`.
                unsafe { self.store(data_addr, u64::zero()) }
            }
            _ => unreachable!(),
        }
    }

    /// Atomically store zero to the side metadata for the given address.
    /// This method mainly facilitates clearing multiple metadata specs for the same address in a loop.
    pub fn set_zero_atomic(&self, data_addr: Address, order: Ordering) {
        use num_traits::Zero;
        match self.log_num_of_bits {
            0..=3 => self.store_atomic(data_addr, u8::zero(), order),
            4 => self.store_atomic(data_addr, u16::zero(), order),
            5 => self.store_atomic(data_addr, u32::zero(), order),
            6 => self.store_atomic(data_addr, u64::zero(), order),
            _ => unreachable!(),
        }
    }

    /// Atomically store one to the side metadata for the data address with the _possible_ side effect of corrupting
    /// and setting the entire byte in the side metadata to 0xff. This can only be used for side metadata smaller
    /// than a byte.
    /// This means it does not only set the side metadata for the data address, and it may also have a side effect of
    /// corrupting and setting the side metadata for the adjacent data addresses. This method is only intended to be
    /// used as an optimization to skip masking and setting bits in some scenarios where setting adjancent bits to 1 is benign.
    ///
    /// # Safety
    /// * `self.log_num_of_bits` must be `< 3`, and `self`'s side-metadata byte for `data_addr`
    ///   must be non-null, mapped, and initialized, with no concurrent non-atomic accesses or live
    ///   non-atomic Rust references.
    /// * This method _may_ overwrite and set all adjacent bits in the same metadata byte to `1` as
    ///   a side effect; the caller must ensure setting adjacent entries in that byte to all-ones is
    ///   sound for the protocol and must not rely on adjacent bits being set (under
    ///   `extreme_assertions`, only `data_addr`'s entry is set).
    pub unsafe fn set_raw_byte_atomic(&self, data_addr: Address, order: Ordering) {
        assert!(self.log_num_of_bits < 3);
        cfg_if::cfg_if! {
            if #[cfg(feature = "extreme_assertions")] {
                // For extreme assertions, we only set 1 to the given address.
                self.store_atomic::<u8>(data_addr, 1, order)
            } else {
                self.side_metadata_access::<false, u8, _, _, _>(
                    data_addr,
                    Some(1u8),
                    || {
                        let meta_addr = address_to_meta_address(self, data_addr);
                        // SAFETY: Caller guarantees `meta_addr` is non-null, mapped, initialized
                        // (`AtomicU8` has alignment 1), and free of concurrent non-atomic accesses
                        // or non-atomic Rust references, and that setting the whole byte to `0xff`
                        // is valid for `self`.
                        unsafe {
                            u8::store_atomic(meta_addr, 0xffu8, order);
                        }
                    },
                    |_| {}
                )
            }
        }
    }

    /// Load the raw byte in the side metadata byte that is mapped to the data address.
    ///
    /// # Safety
    /// * `self`'s side-metadata byte for `data_addr` must be non-null, mapped, and initialized.
    /// * No other thread may concurrently write to or atomically modify that metadata byte, and no
    ///   live `&mut` reference may alias it.
    pub unsafe fn load_raw_byte(&self, data_addr: Address) -> u8 {
        assert!(self.log_num_of_bits < 3);
        self.side_metadata_access::<false, u8, _, _, _>(
            data_addr,
            None,
            || {
                let meta_addr = address_to_meta_address(self, data_addr);
                // SAFETY: Caller guarantees `meta_addr` is non-null, mapped, initialized (`u8` has
                // alignment 1), and free of concurrent writes, atomic modifications, or `&mut` aliases.
                unsafe { meta_addr.load::<u8>() }
            },
            |_| {},
        )
    }

    /// Load the raw word that includes the side metadata byte mapped to the data address.
    ///
    /// # Safety
    /// * The `BYTES_IN_ADDRESS`-aligned side-metadata word containing `data_addr`'s metadata entry
    ///   must be non-null, mapped, and initialized across all `BYTES_IN_ADDRESS` bytes.
    /// * No other thread may concurrently write to or atomically modify any byte in that word, and
    ///   no live `&mut` reference may alias it.
    pub unsafe fn load_raw_word(&self, data_addr: Address) -> usize {
        use crate::util::constants::*;
        assert!(self.log_num_of_bits < (LOG_BITS_IN_BYTE + LOG_BYTES_IN_ADDRESS) as usize);
        self.side_metadata_access::<false, usize, _, _, _>(
            data_addr,
            None,
            || {
                let meta_addr = address_to_meta_address(self, data_addr);
                let aligned_meta_addr = meta_addr.align_down(BYTES_IN_ADDRESS);
                // SAFETY: `aligned_meta_addr` is aligned to `BYTES_IN_ADDRESS == align_of::<usize>()`,
                // and caller guarantees the full word is non-null, mapped, initialized, and free of
                // concurrent writes, atomic modifications, or `&mut` aliases.
                unsafe { aligned_meta_addr.load::<usize>() }
            },
            |_| {},
        )
    }

    /// Stores the new value into the side metadata for the gien address if the current value is the same as the old value.
    /// This method has similar semantics to `compare_exchange` in Rust atomics.
    /// The return value is a result indicating whether the new value was written and containing the previous value.
    /// On success this value is guaranteed to be equal to current.
    pub fn compare_exchange_atomic<T: MetadataValue>(
        &self,
        data_addr: Address,
        old_metadata: T,
        new_metadata: T,
        success_order: Ordering,
        failure_order: Ordering,
    ) -> std::result::Result<T, T> {
        self.side_metadata_access::<true, T, _, _, _>(
            data_addr,
            Some(new_metadata),
            || {
                // SAFETY: By `SideMetadataSpec`'s mapped-region contract and unconditional
                // `assert_spec`, `data_addr`'s metadata cell is mapped, initialized, writable, and
                // accessed atomically without non-atomic races or live non-atomic references.
                match unsafe { self.resolve_atomic::<T>(data_addr) } {
                    ResolvedSideAtomic::SubByte {
                        atomic,
                        lshift,
                        mask,
                    } => {
                        let load_order = load_order_for_rmw(success_order);
                        let real_old_byte = atomic.load(load_order);
                        let expected_old_byte =
                            (real_old_byte & !mask) | ((old_metadata.to_u8().unwrap()) << lshift);
                        let expected_new_byte = (expected_old_byte & !mask)
                            | ((new_metadata.to_u8().unwrap()) << lshift);

                        atomic
                            .compare_exchange(
                                expected_old_byte,
                                expected_new_byte,
                                success_order,
                                failure_order,
                            )
                            .map(|x| FromPrimitive::from_u8((x & mask) >> lshift).unwrap())
                            .map_err(|x| FromPrimitive::from_u8((x & mask) >> lshift).unwrap())
                    }
                    ResolvedSideAtomic::FullWord { atomic } => atomic.compare_exchange_val(
                        old_metadata,
                        new_metadata,
                        success_order,
                        failure_order,
                    ),
                }
            },
            |_res| {
                #[cfg(feature = "extreme_assertions")]
                if _res.is_ok() {
                    sanity::verify_store(self, data_addr, new_metadata);
                }
            },
        )
    }

    /// Safe helper for `fetch_add_atomic`/`fetch_sub_atomic` on sub-byte (`< 8` bits) side metadata.
    fn fetch_ops_on_bits<F: Fn(u8) -> u8>(
        atomic: &AtomicU8,
        lshift: u8,
        mask: u8,
        set_order: Ordering,
        fetch_order: Ordering,
        update: F,
    ) -> u8 {
        let old_raw_byte = atomic
            .fetch_update_val(set_order, fetch_order, |raw_byte: u8| {
                let old_val = (raw_byte & mask) >> lshift;
                let new_val = update(old_val);
                let new_raw_byte = (raw_byte & !mask) | ((new_val << lshift) & mask);
                Some(new_raw_byte)
            })
            .unwrap();
        (old_raw_byte & mask) >> lshift
    }

    /// Adds the value to the current value for this side metadata for the given address.
    /// This method has similar semantics to `fetch_add` in Rust atomics.
    /// Returns the previous value.
    pub fn fetch_add_atomic<T: MetadataValue>(
        &self,
        data_addr: Address,
        val: T,
        order: Ordering,
    ) -> T {
        self.side_metadata_access::<true, T, _, _, _>(
            data_addr,
            Some(val),
            || {
                // SAFETY: By `SideMetadataSpec`'s mapped-region contract and unconditional
                // `assert_spec`, `data_addr`'s metadata cell is mapped, initialized, writable, and
                // accessed solely via atomics.
                match unsafe { self.resolve_atomic::<T>(data_addr) } {
                    ResolvedSideAtomic::SubByte {
                        atomic,
                        lshift,
                        mask,
                    } => {
                        let fetch_order = load_order_for_rmw(order);
                        let prev = Self::fetch_ops_on_bits(
                            atomic,
                            lshift,
                            mask,
                            order,
                            fetch_order,
                            |x| x.wrapping_add(val.to_u8().unwrap()),
                        );
                        FromPrimitive::from_u8(prev).unwrap()
                    }
                    ResolvedSideAtomic::FullWord { atomic } => atomic.fetch_add_val(val, order),
                }
            },
            |_old_val| {
                #[cfg(feature = "extreme_assertions")]
                sanity::verify_update::<T>(self, data_addr, _old_val, _old_val.wrapping_add(&val))
            },
        )
    }

    /// Subtracts the value from the current value for this side metadata for the given address.
    /// This method has similar semantics to `fetch_sub` in Rust atomics.
    /// Returns the previous value.
    pub fn fetch_sub_atomic<T: MetadataValue>(
        &self,
        data_addr: Address,
        val: T,
        order: Ordering,
    ) -> T {
        self.side_metadata_access::<true, T, _, _, _>(
            data_addr,
            Some(val),
            || {
                // SAFETY: By `SideMetadataSpec`'s mapped-region contract and unconditional
                // `assert_spec`, `data_addr`'s metadata cell is mapped, initialized, writable, and
                // accessed solely via atomics.
                match unsafe { self.resolve_atomic::<T>(data_addr) } {
                    ResolvedSideAtomic::SubByte {
                        atomic,
                        lshift,
                        mask,
                    } => {
                        let fetch_order = load_order_for_rmw(order);
                        let prev = Self::fetch_ops_on_bits(
                            atomic,
                            lshift,
                            mask,
                            order,
                            fetch_order,
                            |x| x.wrapping_sub(val.to_u8().unwrap()),
                        );
                        FromPrimitive::from_u8(prev).unwrap()
                    }
                    ResolvedSideAtomic::FullWord { atomic } => atomic.fetch_sub_val(val, order),
                }
            },
            |_old_val| {
                #[cfg(feature = "extreme_assertions")]
                sanity::verify_update::<T>(self, data_addr, _old_val, _old_val.wrapping_sub(&val))
            },
        )
    }

    /// Bitwise 'and' the value with the current value for this side metadata for the given address.
    /// This method has similar semantics to `fetch_and` in Rust atomics.
    /// Returns the previous value.
    pub fn fetch_and_atomic<T: MetadataValue>(
        &self,
        data_addr: Address,
        val: T,
        order: Ordering,
    ) -> T {
        self.side_metadata_access::<true, T, _, _, _>(
            data_addr,
            Some(val),
            || {
                // SAFETY: By `SideMetadataSpec`'s mapped-region contract and unconditional
                // `assert_spec`, `data_addr`'s metadata cell is mapped, initialized, writable, and
                // accessed solely via atomics.
                match unsafe { self.resolve_atomic::<T>(data_addr) } {
                    ResolvedSideAtomic::SubByte {
                        atomic,
                        lshift,
                        mask,
                    } => {
                        let rhs = (val.to_u8().unwrap() << lshift) | !mask;
                        let old_raw_byte = atomic.fetch_and(rhs, order);
                        FromPrimitive::from_u8((old_raw_byte & mask) >> lshift).unwrap()
                    }
                    ResolvedSideAtomic::FullWord { atomic } => atomic.fetch_and_val(val, order),
                }
            },
            |_old_val| {
                #[cfg(feature = "extreme_assertions")]
                sanity::verify_update::<T>(self, data_addr, _old_val, _old_val.bitand(val))
            },
        )
    }

    /// Bitwise 'or' the value with the current value for this side metadata for the given address.
    /// This method has similar semantics to `fetch_or` in Rust atomics.
    /// Returns the previous value.
    pub fn fetch_or_atomic<T: MetadataValue>(
        &self,
        data_addr: Address,
        val: T,
        order: Ordering,
    ) -> T {
        self.side_metadata_access::<true, T, _, _, _>(
            data_addr,
            Some(val),
            || {
                // SAFETY: By `SideMetadataSpec`'s mapped-region contract and unconditional
                // `assert_spec`, `data_addr`'s metadata cell is mapped, initialized, writable, and
                // accessed solely via atomics.
                match unsafe { self.resolve_atomic::<T>(data_addr) } {
                    ResolvedSideAtomic::SubByte {
                        atomic,
                        lshift,
                        mask,
                    } => {
                        let rhs = (val.to_u8().unwrap() << lshift) & mask;
                        let old_raw_byte = atomic.fetch_or(rhs, order);
                        FromPrimitive::from_u8((old_raw_byte & mask) >> lshift).unwrap()
                    }
                    ResolvedSideAtomic::FullWord { atomic } => atomic.fetch_or_val(val, order),
                }
            },
            |_old_val| {
                #[cfg(feature = "extreme_assertions")]
                sanity::verify_update::<T>(self, data_addr, _old_val, _old_val.bitor(val))
            },
        )
    }

    /// Fetches the value for this side metadata for the given address, and applies a function to it that returns an optional new value.
    /// This method has similar semantics to `fetch_update` in Rust atomics.
    /// Returns a Result of Ok(previous_value) if the function returned Some(_), else Err(previous_value).
    pub fn fetch_update_atomic<T: MetadataValue, F: FnMut(T) -> Option<T>>(
        &self,
        data_addr: Address,
        set_order: Ordering,
        fetch_order: Ordering,
        mut f: F,
    ) -> std::result::Result<T, T> {
        // `f` may have side effects (e.g. it may capture and mutate local state), so under
        // `extreme_assertions` we must not call it a second time just to recompute the new
        // value for the sanity check. Instead, stash the new value computed during the actual
        // update here, and have the verify closure read it back.
        #[cfg(feature = "extreme_assertions")]
        let last_new_val: std::cell::Cell<Option<T>> = std::cell::Cell::new(None);
        #[cfg(feature = "extreme_assertions")]
        let last_new_val_ref = &last_new_val;
        self.side_metadata_access::<true, T, _, _, _>(
            data_addr,
            None,
            move || -> std::result::Result<T, T> {
                // SAFETY: By `SideMetadataSpec`'s mapped-region contract and unconditional
                // `assert_spec`, `data_addr`'s metadata cell is mapped, initialized, writable, and
                // accessed solely via atomics across the `fetch_update` loop.
                match unsafe { self.resolve_atomic::<T>(data_addr) } {
                    ResolvedSideAtomic::SubByte {
                        atomic,
                        lshift,
                        mask,
                    } => atomic
                        .fetch_update_val(set_order, fetch_order, |raw_byte: u8| {
                            let old_val = (raw_byte & mask) >> lshift;
                            let new_val = f(FromPrimitive::from_u8(old_val).unwrap());
                            #[cfg(feature = "extreme_assertions")]
                            last_new_val_ref.set(new_val);
                            new_val.map(|new_val| {
                                (raw_byte & !mask) | ((new_val.to_u8().unwrap() << lshift) & mask)
                            })
                        })
                        .map(|x| FromPrimitive::from_u8((x & mask) >> lshift).unwrap())
                        .map_err(|x| FromPrimitive::from_u8((x & mask) >> lshift).unwrap()),
                    ResolvedSideAtomic::FullWord { atomic } => {
                        atomic.fetch_update_val(set_order, fetch_order, |old_val| {
                            let new_val = f(old_val);
                            #[cfg(feature = "extreme_assertions")]
                            last_new_val_ref.set(new_val);
                            new_val
                        })
                    }
                }
            },
            |_result| {
                #[cfg(feature = "extreme_assertions")]
                if let Ok(old_val) = _result {
                    sanity::verify_update::<T>(
                        self,
                        data_addr,
                        old_val,
                        last_new_val_ref.get().unwrap(),
                    )
                }
            },
        )
    }

    /// Search for a data address that has a non zero value in the side metadata. The search starts from the given data address (including this address),
    /// and iterates backwards for the given bytes (non inclusive) before the data address.
    ///
    /// The side metadata for the searched range may not be mapped. Thus when this function checks the given data address, and
    /// when it searches back, it needs to check if the side metadata is mapped or not to avoid loading from an unmapped address.
    /// Note that this function checks whether the side metadata is mapped, rather than whether the data address is mapped.
    /// Some spaces (e.g. `MallocSpace`) have objects in memory that is not mapped by MMTk, but MMTk still maps side metadata for them.
    ///
    /// This function returns an address that is aligned to the region of this side metadata (`log_bytes_per_region`), and the side metadata
    /// for the address is non zero.
    ///
    /// # Safety
    ///
    /// * Any side-metadata pages in the searched window `[data_addr - search_limit_bytes + 1, data_addr]`
    ///   for which `MMAPPER` reports `is_mapped == true` must be readable, initialized, and
    ///   aligned for `T` (`u8` when `self.log_num_of_bits < 3`, or `T::LOG2 == self.log_num_of_bits`).
    /// * No other thread may concurrently write to or atomically modify the searched side metadata
    ///   for the duration of this call, and no live `&mut` reference may alias it.
    #[allow(clippy::let_and_return)]
    pub unsafe fn find_prev_non_zero_value<T: MetadataValue>(
        &self,
        data_addr: Address,
        search_limit_bytes: usize,
    ) -> Option<Address> {
        debug_assert!(search_limit_bytes > 0);

        if self.uses_contiguous_side_metadata() {
            // Contiguous side metadata
            // SAFETY: `self.uses_contiguous_side_metadata()` holds, and caller upholds mapped
            // initialization, alignment, and quiescent non-atomic read preconditions across the
            // searched window.
            let result =
                unsafe { self.find_prev_non_zero_value_fast::<T>(data_addr, search_limit_bytes) };
            #[cfg(debug_assertions)]
            {
                // Double check if the implementation is correct
                // SAFETY: Same preconditions as `find_prev_non_zero_value_fast` above.
                let result2 = unsafe {
                    self.find_prev_non_zero_value_simple::<T>(data_addr, search_limit_bytes)
                };
                assert_eq!(result, result2, "find_prev_non_zero_value_fast returned a diffrent result from the naive implementation.");
            }
            result
        } else {
            // TODO: We should be able to optimize further for this case. However, we need to be careful that the side metadata
            // is not contiguous, and we need to skip to the next chunk's side metadata when we search to a different chunk.
            // This won't be used for VO bit, as VO bit is global and is always contiguous. So for now, I am not bothered to do it.
            warn!("We are trying to search non zero bits in an discontiguous side metadata. The performance is slow, as MMTk does not optimize for this case.");
            // SAFETY: Caller upholds mapped initialization, alignment, and quiescent non-atomic
            // read preconditions across the searched window.
            unsafe { self.find_prev_non_zero_value_simple::<T>(data_addr, search_limit_bytes) }
        }
    }

    /// # Safety
    /// Same preconditions as [`Self::find_prev_non_zero_value`].
    unsafe fn find_prev_non_zero_value_simple<T: MetadataValue>(
        &self,
        data_addr: Address,
        search_limit_bytes: usize,
    ) -> Option<Address> {
        let region_bytes = 1 << self.log_bytes_in_region;
        // Figure out the range that we need to search.
        let start_addr = data_addr.align_down(region_bytes);
        let end_addr = data_addr.saturating_sub(search_limit_bytes) + 1usize;

        let mmap_granularity = MMAPPER.granularity();
        let mut mapped_grain = Address::MAX;

        let mut cursor = start_addr;
        while cursor >= end_addr {
            // We can cache the "is the side metadata for the cursor mapped?" check because MMTk
            // maps metadata at chunk-level
            if cursor < mapped_grain {
                if self.is_mapped(cursor) {
                    mapped_grain = cursor.align_down(mmap_granularity);
                } else {
                    // We encounter an unmapped side metadata. Just return None.
                    return None;
                }
            }
            // If we find non-zero value, just return it.
            // SAFETY: `self.is_mapped(cursor)` verified the chunk's side metadata is mapped, and
            // caller guarantees mapped entries are initialized, aligned for `T`, and not
            // concurrently mutated.
            if !unsafe { self.load::<T>(cursor) }.is_zero() {
                return Some(cursor);
            }
            cursor -= region_bytes;
        }
        None
    }

    /// # Safety
    /// `self.uses_contiguous_side_metadata()` must hold, plus all preconditions of
    /// [`Self::find_prev_non_zero_value`].
    #[allow(clippy::let_and_return)]
    unsafe fn find_prev_non_zero_value_fast<T: MetadataValue>(
        &self,
        data_addr: Address,
        search_limit_bytes: usize,
    ) -> Option<Address> {
        debug_assert!(self.uses_contiguous_side_metadata());

        // Quick check if the side metadata for the data address is mapped at all.
        if !self.is_mapped(data_addr) {
            return None;
        }
        // Quick check if the current data_addr has a non zero value.
        // SAFETY: `self.is_mapped(data_addr)` succeeded above, and caller guarantees mapped
        // metadata is initialized, aligned for `T`, and free of concurrent writes or `&mut` aliases.
        if !unsafe { self.load::<T>(data_addr) }.is_zero() {
            return Some(data_addr.align_down(1 << self.log_bytes_in_region));
        }

        // Figure out the start and end data address.
        let start_addr = data_addr.saturating_sub(search_limit_bytes) + 1usize;
        let end_addr = data_addr;

        // Then figure out the start and end metadata address and bits.
        // The start bit may not be accurate, as we map any address in the region to the same bit.
        // We will filter the result at the end to make sure the found address is in the search range.
        let start_meta_addr = address_to_contiguous_meta_address(self, start_addr);
        let start_meta_shift = meta_byte_lshift(self, start_addr);
        let end_meta_addr = address_to_contiguous_meta_address(self, end_addr);
        let end_meta_shift = meta_byte_lshift(self, end_addr);

        let mut res = None;

        let mut visitor = |range: BitByteRange| {
            match range {
                BitByteRange::Bytes { start, end } => {
                    // SAFETY: Caller guarantees any mapped metadata bytes in `[start, end)` are
                    // initialized and free of concurrent writes, atomic modifications, or `&mut` aliases.
                    let found =
                        unsafe { helpers::find_last_non_zero_bit_in_metadata_bytes(start, end) };
                    match found {
                        helpers::FindMetaBitResult::Found { addr, bit } => {
                            let (addr, bit) = align_metadata_address(self, addr, bit);
                            res = Some(contiguous_meta_address_to_address(self, addr, bit));
                            // Return true to abort the search. We found the bit.
                            true
                        }
                        // If we see unmapped metadata, we don't need to search any more.
                        helpers::FindMetaBitResult::UnmappedMetadata => true,
                        // Return false to continue searching.
                        helpers::FindMetaBitResult::NotFound => false,
                    }
                }
                BitByteRange::BitsInByte {
                    addr,
                    bit_start,
                    bit_end,
                } => {
                    // SAFETY: Caller guarantees that if `addr` is mapped, it is initialized and
                    // free of concurrent writes, atomic modifications, or `&mut` aliases.
                    let found = unsafe {
                        helpers::find_last_non_zero_bit_in_metadata_bits(addr, bit_start, bit_end)
                    };
                    match found {
                        helpers::FindMetaBitResult::Found { addr, bit } => {
                            let (addr, bit) = align_metadata_address(self, addr, bit);
                            res = Some(contiguous_meta_address_to_address(self, addr, bit));
                            // Return true to abort the search. We found the bit.
                            true
                        }
                        // If we see unmapped metadata, we don't need to search any more.
                        helpers::FindMetaBitResult::UnmappedMetadata => true,
                        // Return false to continue searching.
                        helpers::FindMetaBitResult::NotFound => false,
                    }
                }
            }
        };

        ranges::break_bit_range(
            start_meta_addr,
            start_meta_shift,
            end_meta_addr,
            end_meta_shift,
            false,
            &mut visitor,
        );

        // We have to filter the result. We search between [start_addr, end_addr). But we actually
        // search with metadata bits. It is possible the metadata bit for start_addr is the same bit
        // as an address that is before start_addr. E.g. 0x2010f026360 and 0x2010f026361 are mapped
        // to the same bit, 0x2010f026361 is the start address and 0x2010f026360 is outside the search range.
        res.map(|addr| addr.align_down(1 << self.log_bytes_in_region))
            .filter(|addr| *addr >= start_addr && *addr < end_addr)
    }

    /// Search forwards for a data address that has a non zero value in the side metadata. The search starts from the given data
    /// address (including this address), and iterates forwards for the given bytes (non inclusive) before the data address.
    ///
    /// The side metadata for the searched range may not be mapped. Thus when this function checks the given data address, and
    /// when it searches forwards, it needs to check if the side metadata is mapped or not to avoid loading from an unmapped address.
    /// Note that this function checks whether the side metadata is mapped, rather than whether the data address is mapped.
    /// Some spaces (e.g. `MallocSpace`) have objects in memory that is not mapped by MMTk, but MMTk still maps side metadata for them.
    ///
    /// This function returns an address that is aligned to the region of this side metadata (`log_bytes_per_region`), and the side metadata
    /// for the address is non zero.
    ///
    /// # Safety
    ///
    /// * Any side-metadata pages in `[data_addr, data_addr + search_limit_bytes)` (aligned up to
    ///   `1 << self.log_bytes_in_region`) for which `MMAPPER` reports `is_mapped == true` must be
    ///   readable, initialized, and aligned for `T`.
    /// * No other thread may concurrently write to or atomically modify the searched side metadata
    ///   for the duration of this call, and no live `&mut` reference may alias it.
    #[allow(clippy::let_and_return)]
    pub unsafe fn find_next_non_zero_value<T: MetadataValue>(
        &self,
        data_addr: Address,
        search_limit_bytes: usize,
    ) -> Option<Address> {
        debug_assert!(search_limit_bytes > 0);

        if self.uses_contiguous_side_metadata() {
            // Contiguous side metadata
            // SAFETY: `self.uses_contiguous_side_metadata()` holds, and caller upholds mapped
            // initialization, alignment, and quiescent non-atomic read preconditions across the
            // searched window.
            let result =
                unsafe { self.find_next_non_zero_value_fast::<T>(data_addr, search_limit_bytes) };
            #[cfg(debug_assertions)]
            {
                // Double check if the implementation is correct
                // SAFETY: Same preconditions as `find_next_non_zero_value_fast` above.
                let result2 = unsafe {
                    self.find_next_non_zero_value_simple::<T>(data_addr, search_limit_bytes)
                };
                assert_eq!(
                    result,
                    result2,
                    "find_next_non_zero_value_fast returned a different result from the naive implementation. data_addr {}, search_limit_bytes {}",
                    data_addr, search_limit_bytes,
                );
            }
            result
        } else {
            // TODO: We should be able to optimize further for this case. However, we need to be careful that the side metadata
            // is not contiguous, and we need to skip to the next chunk's side metadata when we search to a different chunk.
            // This won't be used for VO bit, as VO bit is global and is always contiguous. So for now, I am not bothered to do it.
            warn!("We are trying to search non zero bits in an discontiguous side metadata. The performance is slow, as MMTk does not optimize for this case.");
            // SAFETY: Caller upholds mapped initialization, alignment, and quiescent non-atomic
            // read preconditions across the searched window.
            unsafe { self.find_next_non_zero_value_simple::<T>(data_addr, search_limit_bytes) }
        }
    }

    /// # Safety
    /// Same preconditions as [`Self::find_next_non_zero_value`].
    unsafe fn find_next_non_zero_value_simple<T: MetadataValue>(
        &self,
        data_addr: Address,
        search_limit_bytes: usize,
    ) -> Option<Address> {
        let region_bytes = 1 << self.log_bytes_in_region;
        // Figure out the range that we need to search.
        let start_addr = data_addr.align_down(region_bytes);
        let end_addr = data_addr + search_limit_bytes;

        let mmap_granularity = MMAPPER.granularity();
        let mut mapped_grain = Address::ZERO;

        let mut cursor = start_addr;
        while cursor < end_addr {
            // We can cache the "is the side metadata for the cursor mapped?" check because MMTk
            // maps metadata at chunk-level
            if cursor > mapped_grain {
                if self.is_mapped(cursor) {
                    mapped_grain = cursor.align_up(mmap_granularity) - 0x1;
                } else {
                    // We encounter an unmapped side metadata. Just return None.
                    return None;
                }
            }
            // If we find non-zero value, just return it.
            // SAFETY: `self.is_mapped(cursor)` verified the chunk's side metadata is mapped, and
            // caller guarantees mapped entries are initialized, aligned for `T`, and not
            // concurrently mutated.
            if !unsafe { self.load::<T>(cursor) }.is_zero() {
                return Some(cursor);
            }
            cursor += region_bytes;
        }
        None
    }

    /// # Safety
    /// `self.uses_contiguous_side_metadata()` must hold, plus all preconditions of
    /// [`Self::find_next_non_zero_value`].
    unsafe fn find_next_non_zero_value_fast<T: MetadataValue>(
        &self,
        data_addr: Address,
        search_limit_bytes: usize,
    ) -> Option<Address> {
        debug_assert!(self.uses_contiguous_side_metadata());

        // Quick check if the side metadata for the data address is mapped at all.
        if !self.is_mapped(data_addr) {
            return None;
        }
        // Quick check if the current data_addr has a non zero value.
        // SAFETY: `self.is_mapped(data_addr)` succeeded above, and caller guarantees mapped
        // metadata is initialized, aligned for `T`, and free of concurrent writes or `&mut` aliases.
        if !unsafe { self.load::<T>(data_addr) }.is_zero() {
            return Some(data_addr.align_down(1 << self.log_bytes_in_region));
        }

        // Figure out the start and end data address.
        let start_addr = data_addr.align_down(1 << self.log_bytes_in_region);
        // We need to align the end_address up because the metadata might be stored right at
        // the end address otherwise. Our loop in `find_first_non_zero_bit_in_metadata_byte`
        // will not load from this end address, resulting in us potentially not finding the
        // correct address for the next set bit.
        let end_addr = (data_addr + search_limit_bytes).align_up(1 << self.log_bytes_in_region);

        // Then figure out the start and end metadata address and bits.
        // The start bit may not be accurate, as we map any address in the region to the same bit.
        // We will filter the result at the end to make sure the found address is in the search range.
        let start_meta_addr = address_to_contiguous_meta_address(self, start_addr);
        let start_meta_shift = meta_byte_lshift(self, start_addr);
        let end_meta_addr = address_to_contiguous_meta_address(self, end_addr);
        let end_meta_shift = meta_byte_lshift(self, end_addr);

        let mut res = None;

        let mut visitor = |range: BitByteRange| {
            match range {
                BitByteRange::Bytes { start, end } => {
                    // SAFETY: Caller guarantees any mapped metadata bytes in `[start, end)` are
                    // initialized and free of concurrent writes, atomic modifications, or `&mut` aliases.
                    let found =
                        unsafe { helpers::find_first_non_zero_bit_in_metadata_bytes(start, end) };
                    match found {
                        helpers::FindMetaBitResult::Found { addr, bit } => {
                            let (addr, bit) = align_metadata_address(self, addr, bit);
                            res = Some(contiguous_meta_address_to_address(self, addr, bit));
                            // Return true to abort the search. We found the bit.
                            true
                        }
                        // If we see unmapped metadata, we don't need to search any more.
                        helpers::FindMetaBitResult::UnmappedMetadata => true,
                        // Return false to continue searching.
                        helpers::FindMetaBitResult::NotFound => false,
                    }
                }
                BitByteRange::BitsInByte {
                    addr,
                    bit_start,
                    bit_end,
                } => {
                    // SAFETY: Caller guarantees that if `addr` is mapped, it is initialized and
                    // free of concurrent writes, atomic modifications, or `&mut` aliases.
                    let found = unsafe {
                        helpers::find_first_non_zero_bit_in_metadata_bits(addr, bit_start, bit_end)
                    };
                    match found {
                        helpers::FindMetaBitResult::Found { addr, bit } => {
                            let (addr, bit) = align_metadata_address(self, addr, bit);
                            res = Some(contiguous_meta_address_to_address(self, addr, bit));
                            // Return true to abort the search. We found the bit.
                            true
                        }
                        // If we see unmapped metadata, we don't need to search any more.
                        helpers::FindMetaBitResult::UnmappedMetadata => true,
                        // Return false to continue searching.
                        helpers::FindMetaBitResult::NotFound => false,
                    }
                }
            }
        };

        ranges::break_bit_range(
            start_meta_addr,
            start_meta_shift,
            end_meta_addr,
            end_meta_shift,
            true,
            &mut visitor,
        );

        // We have to filter the result. We search between [start_addr, end_addr). But we actually
        // search with metadata bits. It is possible the metadata bit for start_addr is the same bit
        // as an address that is before start_addr. E.g. 0x2010f026360 and 0x2010f026361 are mapped
        // to the same bit, 0x2010f026361 is the start address and 0x2010f026360 is outside the search range.
        res.map(|addr| addr.align_down(1 << self.log_bytes_in_region))
            .filter(|addr| *addr >= start_addr && *addr < end_addr)
    }

    /// Search for data addresses that have non zero values in the side metadata.  This method is
    /// primarily used for heap traversal by scanning the VO bits.
    ///
    /// This function searches the side metadata for the data address range from `data_start_addr`
    /// (inclusive) to `data_end_addr` (exclusive).  The data address range must be fully mapped.
    ///
    /// For each data region that has non-zero side metadata, `visit_data` is called with the lowest
    /// address of that region.  Note that it may not be the original address used to set the
    /// metadata bits.
    ///
    /// # Safety
    ///
    /// * The side-metadata entries covering `[data_start_addr, data_end_addr)` under `self` must be
    ///   non-null (whenever `data_start_addr < data_end_addr`), fully mapped, readable,
    ///   initialized, and aligned for `T` (when `!(self.uses_contiguous_side_metadata() && self.log_num_of_bits == 0)`).
    /// * Neither another thread nor `visit_data` may concurrently write to or atomically modify the
    ///   scanned side-metadata range (including shared boundary bytes) while this scan is running,
    ///   and no live `&mut` reference may alias it.
    pub unsafe fn scan_non_zero_values<T: MetadataValue>(
        &self,
        data_start_addr: Address,
        data_end_addr: Address,
        visit_data: &mut impl FnMut(Address),
    ) {
        if self.uses_contiguous_side_metadata() && self.log_num_of_bits == 0 {
            // Contiguous one-bit-per-region side metadata
            // TODO: VO bits is one-bit-per-word.  But if we want to scan other metadata (such as
            // the forwarding bits which has two bits per word), we will need to refactor the
            // algorithm of `scan_non_zero_values_fast`.
            // SAFETY: `self.uses_contiguous_side_metadata() && self.log_num_of_bits == 0` holds,
            // and caller guarantees `[data_start_addr, data_end_addr)` has fully mapped,
            // initialized, quiescent side metadata without live `&mut` aliases.
            unsafe {
                self.scan_non_zero_values_fast(data_start_addr, data_end_addr, visit_data);
            }
        } else {
            // TODO: VO bits are always contiguous.  But if we want to scan other metadata, such as
            // side mark bits, we need to refactor `bulk_update_metadata` to support `FnMut`, too,
            // and use it to apply `scan_non_zero_values_fast` on each contiguous side metadata
            // range.
            warn!(
                "We are trying to search for non zero bits in a discontiguous side metadata \
            or the metadata has more than one bit per region. \
                The performance is slow, as MMTk does not optimize for this case."
            );
            // SAFETY: Caller guarantees `[data_start_addr, data_end_addr)` has fully mapped,
            // initialized, `T`-aligned, quiescent side metadata without live `&mut` aliases.
            unsafe {
                self.scan_non_zero_values_simple::<T>(data_start_addr, data_end_addr, visit_data);
            }
        }
    }

    /// # Safety
    /// * Every region in `[data_start_addr, data_end_addr)` must have mapped, initialized, non-null
    ///   side metadata aligned for `T`.
    /// * Neither another thread nor `visit_data` may mutate the scanned side metadata while this
    ///   scan is running, and no live `&mut` reference may alias it.
    unsafe fn scan_non_zero_values_simple<T: MetadataValue>(
        &self,
        data_start_addr: Address,
        data_end_addr: Address,
        visit_data: &mut impl FnMut(Address),
    ) {
        let region_bytes = 1usize << self.log_bytes_in_region;

        let mut cursor = data_start_addr;
        while cursor < data_end_addr {
            debug_assert!(self.is_mapped(cursor));

            // If we find non-zero value, just call back.
            // SAFETY: Caller guarantees `cursor` in `[data_start_addr, data_end_addr)` has mapped,
            // initialized, `T`-aligned side metadata with no concurrent writes or `&mut` aliases.
            if !unsafe { self.load::<T>(cursor) }.is_zero() {
                visit_data(cursor);
            }
            cursor += region_bytes;
        }
    }

    /// # Safety
    /// * `self.uses_contiguous_side_metadata()` and `self.log_num_of_bits == 0` must hold.
    /// * The contiguous side-metadata range covering `[data_start_addr, data_end_addr)` must be
    ///   non-null, mapped, and initialized.
    /// * Neither another thread nor `visit_data` may mutate the scanned side metadata while this
    ///   scan is running, and no live `&mut` reference may alias it.
    unsafe fn scan_non_zero_values_fast(
        &self,
        data_start_addr: Address,
        data_end_addr: Address,
        visit_data: &mut impl FnMut(Address),
    ) {
        debug_assert!(self.uses_contiguous_side_metadata());
        debug_assert_eq!(self.log_num_of_bits, 0);

        // Then figure out the start and end metadata address and bits.
        let start_meta_addr = address_to_contiguous_meta_address(self, data_start_addr);
        let start_meta_shift = meta_byte_lshift(self, data_start_addr);
        let end_meta_addr = address_to_contiguous_meta_address(self, data_end_addr);
        let end_meta_shift = meta_byte_lshift(self, data_end_addr);

        let mut visitor = |range| {
            match range {
                BitByteRange::Bytes { start, end } => {
                    // SAFETY: Caller guarantees `[start, end)` lies within the mapped, initialized,
                    // quiescent contiguous side-metadata range for `[data_start_addr, data_end_addr)`
                    // without concurrent mutations (including inside `visit_data`) or `&mut` aliases.
                    unsafe {
                        helpers::scan_non_zero_bits_in_metadata_bytes(
                            start,
                            end,
                            &mut |addr, bit| {
                                visit_data(helpers::contiguous_meta_address_to_address(
                                    self, addr, bit,
                                ));
                            },
                        );
                    }
                }
                BitByteRange::BitsInByte {
                    addr,
                    bit_start,
                    bit_end,
                } => {
                    // SAFETY: Caller guarantees `addr` lies in the mapped, initialized, quiescent
                    // side-metadata range for `[data_start_addr, data_end_addr)` without concurrent
                    // mutations or live `&mut` aliases.
                    unsafe {
                        helpers::scan_non_zero_bits_in_metadata_bits(
                            addr,
                            bit_start,
                            bit_end,
                            &mut |addr, bit| {
                                visit_data(helpers::contiguous_meta_address_to_address(
                                    self, addr, bit,
                                ));
                            },
                        )
                    }
                }
            }
            false
        };

        ranges::break_bit_range(
            start_meta_addr,
            start_meta_shift,
            end_meta_addr,
            end_meta_shift,
            true,
            &mut visitor,
        );
    }
}

impl fmt::Debug for SideMetadataSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!(
            "SideMetadataSpec {} {{ \
            **is_global: {:?} \
            **offset: 0x{:x} \
            **log_num_of_bits: 0x{:x} \
            **log_bytes_in_region: 0x{:x} \
            }}",
            self.name, self.is_global, self.offset, self.log_num_of_bits, self.log_bytes_in_region
        ))
    }
}

/// Calculate the offset of the next side metadata spec after the given spec.
/// This is used to calculate the offset field in [`crate::util::metadata::side_metadata::SideMetadataSpec`].
pub const fn side_metadata_offset_after(spec: &SideMetadataSpec) -> usize {
    // Some metadata may be so small that its size is not a multiple of word/u64 alignment (e.g.,
    // `CHUNK_MARK` on 32-bit targets where side metadata is allocated per chunk). Aligning up to
    // `SIDE_METADATA_SPEC_ALIGN` (`max(BYTES_IN_WORD, align_of::<AtomicU64>())` = 8 bytes) ensures
    // subsequent specs — including 64-bit `AtomicU64` metadata on 32-bit architectures — are
    // always properly aligned.
    raw_align_up(spec.upper_bound_offset(), SIDE_METADATA_SPEC_ALIGN)
}

/// This struct stores all the side metadata specs for a policy. Generally a policy needs to know its own
/// side metadata spec as well as the plan's specs.
pub(crate) struct SideMetadataContext {
    // For plans
    pub global: Vec<SideMetadataSpec>,
    // For policies
    pub local: Vec<SideMetadataSpec>,
}

impl SideMetadataContext {
    #[allow(clippy::vec_init_then_push)] // allow this, as we conditionally push based on features.
    pub fn new_global_specs(specs: &[SideMetadataSpec]) -> Vec<SideMetadataSpec> {
        let mut ret = vec![];

        #[cfg(feature = "vo_bit")]
        ret.push(VO_BIT_SIDE_METADATA_SPEC);

        if let Some(spec) = crate::mmtk::SFT_MAP.get_side_metadata() {
            if spec.is_global {
                ret.push(*spec);
            }
        }

        // Any plan that uses the chunk map needs to reserve the chunk map table.
        // As we use either the mark sweep or (non moving) immix as the non moving space,
        // and both policies use the chunk map, we just add the chunk map table globally.
        ret.push(crate::util::heap::chunk_map::ChunkMap::ALLOC_TABLE);

        ret.extend_from_slice(specs);
        ret
    }

    pub fn get_local_specs(&self) -> &[SideMetadataSpec] {
        &self.local
    }

    #[cfg(debug_assertions)]
    pub fn assert_metadata_ranges_in_reserved_range(
        &self,
        start: Address,
        size: usize,
        space_name: &str,
    ) {
        let reserved = {
            let base = super::layout::global_side_metadata_base_address();
            let bytes = super::layout::side_metadata_reserved_bytes();
            base..(base + bytes)
        };
        let check_spec = |spec: &SideMetadataSpec| {
            if !spec.uses_contiguous_side_metadata() {
                return;
            }
            let metadata_start = address_to_meta_address(spec, start);
            let mmap_start = metadata_start.align_down(BYTES_IN_PAGE);
            let metadata_size = data_to_meta_size_round_up(spec, size);
            let mmap_end = (metadata_start + metadata_size).align_up(BYTES_IN_PAGE);
            debug_assert!(
                mmap_start >= reserved.start && mmap_end <= reserved.end,
                "Side metadata range for spec {} in space {} is outside reserved range: [{}, {}) vs [{}, {})",
                spec.name,
                space_name,
                mmap_start,
                mmap_end,
                reserved.start,
                reserved.end
            );
        };
        self.global.iter().for_each(check_spec);
        self.local.iter().for_each(check_spec);
    }

    /// Return the pages reserved for side metadata based on the data pages we used.
    // We used to use PageAccouting to count pages used in side metadata. However,
    // that means we always count pages while we may reserve less than a page each time.
    // This could lead to overcount. I think the easier way is to not account
    // when we allocate for sidemetadata, but to calculate the side metadata usage based on
    // how many data pages we use when reporting.
    pub fn calculate_reserved_pages(&self, data_pages: usize) -> usize {
        let mut total = 0;
        for spec in self.global.iter() {
            // This rounds up.  No matter how small `data_pages` is, the side metadata size will be
            // at least one page.  This behavior is *intended*.  This over-estimated amount is used
            // for triggering GC and resizing the heap.
            total += data_to_meta_size_round_up(spec, data_pages);
        }
        for spec in self.local.iter() {
            total += data_to_meta_size_round_up(spec, data_pages);
        }
        total
    }

    // ** NOTE: **
    //  Regardless of the number of bits in a metadata unit, we always represent its content as a word.

    /// Tries to map the required metadata space and returns `true` is successful.
    /// This can be called at page granularity.
    pub fn try_map_metadata_space(
        &self,
        start: Address,
        size: usize,
        space_name: &str,
    ) -> MmapResult<()> {
        debug!(
            "try_map_metadata_space({}, 0x{:x}, {}, {})",
            start,
            size,
            self.global.len(),
            self.local.len()
        );
        // Page aligned
        debug_assert!(start.is_aligned_to(BYTES_IN_PAGE));
        debug_assert!(size % BYTES_IN_PAGE == 0);
        self.map_metadata_internal(start, size, false, space_name)
    }

    /// Tries to map the required metadata address range, without reserving swap-space/physical memory for it.
    /// This will make sure the address range is exclusive to the caller. This should be called at chunk granularity.
    ///
    /// NOTE: Accessing addresses in this range will produce a segmentation fault if swap-space is not mapped using the `try_map_metadata_space` function.
    pub fn try_map_metadata_address_range(
        &self,
        start: Address,
        size: usize,
        name: &str,
    ) -> MmapResult<()> {
        debug!(
            "try_map_metadata_address_range({}, 0x{:x}, {}, {})",
            start,
            size,
            self.global.len(),
            self.local.len()
        );
        // Chunk aligned
        debug_assert!(start.is_aligned_to(BYTES_IN_CHUNK));
        debug_assert!(size % BYTES_IN_CHUNK == 0);
        self.map_metadata_internal(start, size, true, name)
    }

    /// The internal function to mmap metadata
    ///
    /// # Arguments
    /// * `start` - The starting address of the source data.
    /// * `size` - The size of the source data (in bytes).
    /// * `no_reserve` - whether to invoke mmap with a noreserve flag (we use this flag to quarantine address range)
    /// * `space_name`: The name of the space, used for annotating the mmap.
    fn map_metadata_internal(
        &self,
        start: Address,
        size: usize,
        no_reserve: bool,
        space_name: &str,
    ) -> MmapResult<()> {
        for spec in self.global.iter() {
            let anno = MmapAnnotation::SideMeta {
                space: space_name,
                meta: spec.name,
            };
            try_mmap_contiguous_metadata_space(start, size, spec, no_reserve, &anno)?;
        }

        #[cfg(target_pointer_width = "32")]
        let mut lsize: usize = 0;

        for spec in self.local.iter() {
            // For local side metadata, we always have to reserve address space for all local
            // metadata required by all policies in MMTk to be able to calculate a constant offset
            // for each local metadata at compile-time (it's like assigning an ID to each policy).
            //
            // As the plan is chosen at run-time, we will never know which subset of policies will
            // be used during run-time. We can't afford this much address space in 32-bits.
            // So, we switch to the chunk-based approach for this specific case.
            //
            // The global metadata is different in that for each plan, we can calculate its constant
            // base addresses at compile-time. Using the chunk-based approach will need the same
            // address space size as the current not-chunked approach.
            #[cfg(target_pointer_width = "64")]
            {
                let anno = MmapAnnotation::SideMeta {
                    space: space_name,
                    meta: spec.name,
                };
                try_mmap_contiguous_metadata_space(start, size, spec, no_reserve, &anno)?;
            }
            #[cfg(target_pointer_width = "32")]
            {
                lsize += metadata_bytes_per_chunk(spec.log_bytes_in_region, spec.log_num_of_bits);
            }
        }

        #[cfg(target_pointer_width = "32")]
        if lsize > 0 {
            let max = BYTES_IN_CHUNK >> super::layout::LOG_LOCAL_SIDE_METADATA_WORST_CASE_RATIO;
            debug_assert!(
                lsize <= max,
                "local side metadata per chunk (0x{:x}) must be less than (0x{:x})",
                lsize,
                max
            );
            // We are creating a mmap for all side metadata instead of one specific metadata.  We
            // just annotate it as "all" here.
            let anno = MmapAnnotation::SideMeta {
                space: space_name,
                meta: "all",
            };
            try_map_per_chunk_metadata_space(start, size, lsize, no_reserve, &anno)?;
        }

        Ok(())
    }

    /// Unmap the corresponding metadata space or panic.
    ///
    /// Note-1: This function is only used for test and debug right now.
    ///
    /// Note-2: This function uses munmap() which works at page granularity.
    ///     If the corresponding metadata space's size is not a multiple of page size,
    ///     the actual unmapped space will be bigger than what you specify.
    #[cfg(test)]
    pub fn ensure_unmap_metadata_space(&self, start: Address, size: usize) {
        trace!("ensure_unmap_metadata_space({}, 0x{:x})", start, size);
        debug_assert!(start.is_aligned_to(BYTES_IN_PAGE));
        debug_assert!(size % BYTES_IN_PAGE == 0);

        for spec in self.global.iter() {
            ensure_munmap_contiguous_metadata_space(start, size, spec);
        }

        for spec in self.local.iter() {
            #[cfg(target_pointer_width = "64")]
            {
                ensure_munmap_contiguous_metadata_space(start, size, spec);
            }
            #[cfg(target_pointer_width = "32")]
            {
                ensure_munmap_chunked_metadata_space(start, size, spec);
            }
        }
    }
}

/// A byte array in side-metadata
pub struct MetadataByteArrayRef<const ENTRIES: usize> {
    #[cfg(feature = "extreme_assertions")]
    heap_range_start: Address,
    #[cfg(feature = "extreme_assertions")]
    spec: SideMetadataSpec,
    meta_start: Address,
}

impl<const ENTRIES: usize> MetadataByteArrayRef<ENTRIES> {
    /// Get a piece of metadata address range as a byte array.
    ///
    /// # Arguments
    ///
    /// * `metadata_spec` - The specification of the target side metadata.
    /// * `start` - The starting address of the heap range.
    /// * `bytes` - The size of the heap range.
    ///
    pub fn new(metadata_spec: &SideMetadataSpec, start: Address, bytes: usize) -> Self {
        debug_assert_eq!(
            metadata_spec.log_num_of_bits, LOG_BITS_IN_BYTE as usize,
            "Each heap entry should map to a byte in side-metadata"
        );
        debug_assert_eq!(
            bytes >> metadata_spec.log_bytes_in_region,
            ENTRIES,
            "Heap range size and MetadataByteArray size does not match"
        );
        Self {
            #[cfg(feature = "extreme_assertions")]
            heap_range_start: start,
            #[cfg(feature = "extreme_assertions")]
            spec: *metadata_spec,
            meta_start: address_to_meta_address(metadata_spec, start),
        }
    }

    /// Get the length of the array.
    #[allow(clippy::len_without_is_empty)]
    pub const fn len(&self) -> usize {
        ENTRIES
    }

    /// Get a byte from the metadata byte array at the given index.
    #[allow(clippy::let_and_return)]
    pub fn get(&self, index: usize) -> u8 {
        assert!(
            index < ENTRIES,
            "index out of bounds: the len is {ENTRIES} but the index is {index}"
        );
        #[cfg(feature = "extreme_assertions")]
        let _lock = sanity::SANITY_LOCK.lock().unwrap();
        let entry_addr = self.meta_start + index;
        // SAFETY: `MetadataByteArrayRef` (preserved with its public signature; e.g. Immix
        // `Block::line_mark_table`) requires the `ENTRIES` 1-byte side-metadata entries starting
        // at `self.meta_start` for `[start, start + bytes)` to be non-null, mapped, initialized,
        // and not concurrently mutated when `get` is called. `assert!(index < ENTRIES)` bounds
        // `entry_addr` within `[self.meta_start, self.meta_start + ENTRIES)` (`u8` has alignment 1).
        // Holding `meta_start: Address` rather than `&'static [u8; ENTRIES]` avoids UB from
        // asserting `'static` immutability over side-metadata bytes that are mutated between reads.
        let value = unsafe { entry_addr.load::<u8>() };
        #[cfg(feature = "extreme_assertions")]
        {
            let data_addr = self.heap_range_start + (index << self.spec.log_bytes_in_region);
            sanity::verify_load::<u8>(&self.spec, data_addr, value);
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mmap_anno_test;
    use crate::util::metadata::side_metadata::SideMetadataContext;

    // offset is not used in these tests.
    pub const ZERO_OFFSET: usize = 0;

    #[test]
    fn calculate_reserved_pages_one_spec() {
        // 1 bit per 8 bytes - 1:64
        let spec = SideMetadataSpec {
            name: "test_spec",
            is_global: true,
            offset: ZERO_OFFSET,
            log_num_of_bits: 0,
            log_bytes_in_region: 3,
        };
        let side_metadata = SideMetadataContext {
            global: vec![spec],
            local: vec![],
        };
        assert_eq!(side_metadata.calculate_reserved_pages(0), 0);
        assert_eq!(side_metadata.calculate_reserved_pages(63), 1);
        assert_eq!(side_metadata.calculate_reserved_pages(64), 1);
        assert_eq!(side_metadata.calculate_reserved_pages(65), 2);
        assert_eq!(side_metadata.calculate_reserved_pages(1024), 16);
    }

    #[test]
    fn calculate_reserved_pages_multi_specs() {
        // 1 bit per 8 bytes - 1:64
        let gspec = SideMetadataSpec {
            name: "gspec",
            is_global: true,
            offset: ZERO_OFFSET,
            log_num_of_bits: 0,
            log_bytes_in_region: 3,
        };
        // 2 bits per page - 2 / (4k * 8) = 1:16k
        let lspec = SideMetadataSpec {
            name: "lspec",
            is_global: false,
            offset: ZERO_OFFSET,
            log_num_of_bits: 1,
            log_bytes_in_region: 12,
        };
        let side_metadata = SideMetadataContext {
            global: vec![gspec],
            local: vec![lspec],
        };
        assert_eq!(side_metadata.calculate_reserved_pages(1024), 16 + 1);
    }

    use crate::util::heap::layout::vm_layout;
    use crate::util::test_util::{serial_test, with_cleanup};
    use paste::paste;

    const TEST_LOG_BYTES_IN_REGION: usize = 12;

    fn test_side_metadata(
        log_bits: usize,
        f: impl Fn(&SideMetadataSpec, Address, Address) + std::panic::RefUnwindSafe,
    ) {
        serial_test(|| {
            core_test_initialize_side_metadata();

            let spec = SideMetadataSpec {
                name: "Test Spec $tname",
                is_global: true,
                offset: 0,
                log_num_of_bits: log_bits,
                log_bytes_in_region: TEST_LOG_BYTES_IN_REGION, // page size
            };
            let context = SideMetadataContext {
                global: vec![spec],
                local: vec![],
            };
            let mut sanity = SideMetadataSanity::new();
            sanity.verify_metadata_context("TestPolicy", &context);

            let data_addr = vm_layout::vm_layout().heap_start;
            // Make sure the address is mapped.
            crate::MMAPPER
                .ensure_mapped(
                    data_addr,
                    1,
                    HugePageSupport::No,
                    MmapProtection::ReadWrite,
                    mmap_anno_test!(),
                )
                .unwrap();
            let meta_addr = address_to_meta_address(&spec, data_addr);
            with_cleanup(
                || {
                    let mmap_result =
                        context.try_map_metadata_space(data_addr, BYTES_IN_PAGE, "test_space");
                    assert!(mmap_result.is_ok(), "{:?}", mmap_result);

                    f(&spec, data_addr, meta_addr);
                },
                || {
                    // Clear the metadata -- use u64 (max length we support)
                    assert!(log_bits <= 6);
                    // SAFETY: `try_map_metadata_space` mapped at least one 4096-byte page starting
                    // at page-aligned `meta_addr` (`>= align_of::<u64>()`), and `serial_test`
                    // guarantees exclusive single-threaded access with no aliasing references.
                    unsafe {
                        meta_addr.store::<u64>(0);
                    }

                    sanity::reset();
                },
            )
        })
    }

    fn max_value(log_bits: usize) -> u64 {
        (0..(1 << log_bits)).fold(0, |accum, x| accum + (1 << x))
    }
    #[test]
    fn test_max_value() {
        assert_eq!(max_value(0), 1);
        assert_eq!(max_value(1), 0b11);
        assert_eq!(max_value(2), 0b1111);
        assert_eq!(max_value(3), 255);
        assert_eq!(max_value(4), 65535);
    }

    macro_rules! test_side_metadata_access {
        ($tname: ident, $type: ty, $log_bits: expr) => {
            paste!{
                #[test]
                fn [<$tname _load>]() {
                    test_side_metadata($log_bits, |spec, data_addr, meta_addr| {
                        // Initial value should be 0
                        // SAFETY: `test_side_metadata` mapped and zero-initialized `spec`'s page
                        // at `meta_addr` for `data_addr` inside single-threaded `serial_test`.
                        assert_eq!(unsafe { spec.load::<$type>(data_addr) }, 0);
                        assert_eq!(spec.load_atomic::<$type>(data_addr, Ordering::SeqCst), 0);

                        // Set to max
                        let max_value: $type = max_value($log_bits) as _;
                        // SAFETY: Single-threaded `serial_test` holds exclusive access to mapped,
                        // `$type`-aligned metadata for `data_addr`.
                        unsafe {
                            spec.store::<$type>(data_addr, max_value);
                        }
                        // SAFETY: Exclusive single-threaded access to mapped, initialized metadata.
                        assert_eq!(unsafe { spec.load::<$type>(data_addr) }, max_value);
                        assert_eq!(spec.load_atomic::<$type>(data_addr, Ordering::SeqCst), max_value);
                        // SAFETY: `meta_addr` is page-aligned (`>= align_of::<$type>()`), mapped,
                        // initialized, and read sequentially inside `serial_test`.
                        assert_eq!(unsafe { meta_addr.load::<$type>() }, max_value);
                    });
                }

                #[test]
                fn [<$tname _store>]() {
                    test_side_metadata($log_bits, |spec, data_addr, meta_addr| {
                        let max_value: $type = max_value($log_bits) as _;

                        // Set the metadata byte(s) to all 1s
                        // SAFETY: `meta_addr` is page-aligned (`>= align_of::<$type>()`), mapped
                        // read-write, and exclusively accessed inside `serial_test`.
                        unsafe {
                            meta_addr.store::<$type>(<$type>::MAX);
                        }
                        // Store 0 to the side metadata
                        // SAFETY: Exclusive single-threaded access to mapped, initialized metadata.
                        unsafe {
                            spec.store::<$type>(data_addr, 0);
                        }
                        // SAFETY: Exclusive single-threaded access to mapped, initialized metadata.
                        assert_eq!(unsafe { spec.load::<$type>(data_addr) }, 0);
                        // Only the affected bits are set to 0
                        // SAFETY: `meta_addr` is page-aligned, mapped, initialized, and exclusively accessed.
                        assert_eq!(unsafe { meta_addr.load::<$type>() }, <$type>::MAX & (!max_value));
                    });
                }

                #[test]
                fn [<$tname _atomic_store>]() {
                    test_side_metadata($log_bits, |spec, data_addr, meta_addr| {
                        let max_value: $type = max_value($log_bits) as _;

                        // Set the metadata byte(s) to all 1s
                        // SAFETY: `meta_addr` is page-aligned (`>= align_of::<$type>()`), mapped
                        // read-write, and written sequentially before `store_atomic` in `serial_test`.
                        unsafe {
                            meta_addr.store::<$type>(<$type>::MAX);
                        }
                        // Store 0 to the side metadata
                        spec.store_atomic::<$type>(data_addr, 0, Ordering::SeqCst);
                        // SAFETY: `store_atomic` completed sequentially in single-threaded
                        // `serial_test`; `data_addr`'s metadata is mapped, initialized, and
                        // unaliased for cross-checking via non-atomic `spec.load`.
                        assert_eq!(unsafe { spec.load::<$type>(data_addr) }, 0);
                        // Only the affected bits are set to 0
                        // SAFETY: `meta_addr` is page-aligned, mapped, initialized, and read
                        // sequentially after `store_atomic` inside single-threaded `serial_test`.
                        assert_eq!(unsafe { meta_addr.load::<$type>() }, <$type>::MAX & (!max_value));
                    });
                }

                #[test]
                fn [<$tname _compare_exchange_success>]() {
                    test_side_metadata($log_bits, |spec, data_addr, meta_addr| {
                        let max_value: $type = max_value($log_bits) as _;
                        // Set the metadata byte(s) to all 1s
                        // SAFETY: `meta_addr` is page-aligned, mapped read-write, and written
                        // sequentially inside single-threaded `serial_test`.
                        unsafe {
                            meta_addr.store::<$type>(<$type>::MAX);
                        }
                        // Store 1 to the side metadata
                        spec.store_atomic::<$type>(data_addr, 1, Ordering::SeqCst);

                        let old_val = spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        assert_eq!(old_val, 1);

                        let new_val = 0;
                        let res = spec.compare_exchange_atomic::<$type>(data_addr, old_val, new_val, Ordering::SeqCst, Ordering::SeqCst);
                        assert!(res.is_ok());
                        assert_eq!(res.unwrap(), old_val, "old vals do not match");

                        let after_update = spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        assert_eq!(after_update, new_val);
                        // Only the affected bits are set to 0
                        // SAFETY: `meta_addr` is page-aligned, mapped, initialized, and read
                        // sequentially inside single-threaded `serial_test`.
                        assert_eq!(unsafe { meta_addr.load::<$type>() }, <$type>::MAX & (!max_value));
                    });
                }

                #[test]
                fn [<$tname _compare_exchange_fail>]() {
                    test_side_metadata($log_bits, |spec, data_addr, meta_addr| {
                        // Set the metadata byte(s) to all 1s
                        // SAFETY: `meta_addr` is page-aligned, mapped read-write, and written
                        // sequentially inside single-threaded `serial_test`.
                        unsafe {
                            meta_addr.store::<$type>(<$type>::MAX);
                        }
                        // Store 1 to the side metadata
                        spec.store_atomic::<$type>(data_addr, 1, Ordering::SeqCst);

                        let old_val = spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        assert_eq!(old_val, 1);

                        // make old_val outdated
                        spec.store_atomic::<$type>(data_addr, 0, Ordering::SeqCst);
                        // SAFETY: `meta_addr` is page-aligned, mapped, initialized, and read
                        // sequentially inside single-threaded `serial_test`.
                        let bits_before_cas = unsafe { meta_addr.load::<$type>() };

                        let new_val = 0;
                        let res = spec.compare_exchange_atomic::<$type>(data_addr, old_val, new_val, Ordering::SeqCst, Ordering::SeqCst);
                        assert!(res.is_err());
                        assert_eq!(res.err().unwrap(), 0);
                        // SAFETY: `meta_addr` is page-aligned, mapped, initialized, and read
                        // sequentially inside single-threaded `serial_test`.
                        let bits_after_cas = unsafe { meta_addr.load::<$type>() };
                        assert_eq!(bits_before_cas, bits_after_cas);
                    });
                }

                #[test]
                fn [<$tname _fetch_add_1>]() {
                    test_side_metadata($log_bits, |spec, data_addr, meta_addr| {
                        // Set the metadata byte(s) to all 1s
                        // SAFETY: `meta_addr` is page-aligned, mapped read-write, and written
                        // sequentially inside single-threaded `serial_test`.
                        unsafe {
                            meta_addr.store::<$type>(<$type>::MAX);
                        }
                        // Store 0 to the side metadata
                        spec.store_atomic::<$type>(data_addr, 0, Ordering::SeqCst);

                        let old_val = spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);

                        let old_val_from_fetch = spec.fetch_add_atomic::<$type>(data_addr, 1, Ordering::SeqCst);
                        assert_eq!(old_val_from_fetch, old_val);

                        let new_val = spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        assert_eq!(new_val, 1);
                    });
                }

                #[test]
                fn [<$tname _fetch_add_max>]() {
                    test_side_metadata($log_bits, |spec, data_addr, meta_addr| {
                        let max_value: $type = max_value($log_bits) as _;
                        // Set the metadata byte(s) to all 1s
                        // SAFETY: `meta_addr` is page-aligned, mapped read-write, and written
                        // sequentially inside single-threaded `serial_test`.
                        unsafe {
                            meta_addr.store::<$type>(<$type>::MAX);
                        }
                        // Store 0 to the side metadata
                        spec.store_atomic::<$type>(data_addr, 0, Ordering::SeqCst);

                        let old_val = spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);

                        let old_val_from_fetch = spec.fetch_add_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);
                        assert_eq!(old_val_from_fetch, old_val);

                        let new_val = spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        assert_eq!(new_val, max_value);
                    });
                }

                #[test]
                fn [<$tname _fetch_add_overflow>]() {
                    test_side_metadata($log_bits, |spec, data_addr, meta_addr| {
                        let max_value: $type = max_value($log_bits) as _;
                        // Set the metadata byte(s) to all 1s
                        // SAFETY: `meta_addr` is page-aligned, mapped read-write, and written
                        // sequentially inside single-threaded `serial_test`.
                        unsafe {
                            meta_addr.store::<$type>(<$type>::MAX);
                        }
                        // Store max to the side metadata
                        spec.store_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);

                        let old_val = spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);

                        // add 1 to max value will cause overflow and wrap around to 0
                        let old_val_from_fetch = spec.fetch_add_atomic::<$type>(data_addr, 1, Ordering::SeqCst);
                        assert_eq!(old_val_from_fetch, old_val);

                        let new_val = spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        assert_eq!(new_val, 0);
                    });
                }

                #[test]
                fn [<$tname _fetch_sub_1>]() {
                    test_side_metadata($log_bits, |spec, data_addr, meta_addr| {
                        // Set the metadata byte(s) to all 1s
                        // SAFETY: `meta_addr` is page-aligned, mapped read-write, and written
                        // sequentially inside single-threaded `serial_test`.
                        unsafe {
                            meta_addr.store::<$type>(<$type>::MAX);
                        }
                        // Store 1 to the side metadata
                        spec.store_atomic::<$type>(data_addr, 1, Ordering::SeqCst);

                        let old_val = spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);

                        let old_val_from_fetch = spec.fetch_sub_atomic::<$type>(data_addr, 1, Ordering::SeqCst);
                        assert_eq!(old_val_from_fetch, old_val);

                        let new_val = spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        assert_eq!(new_val, 0);
                    });
                }

                #[test]
                fn [<$tname _fetch_sub_max>]() {
                    test_side_metadata($log_bits, |spec, data_addr, meta_addr| {
                        let max_value: $type = max_value($log_bits) as _;
                        // Set the metadata byte(s) to all 1s
                        // SAFETY: `meta_addr` is page-aligned, mapped read-write, and written
                        // sequentially inside single-threaded `serial_test`.
                        unsafe {
                            meta_addr.store::<$type>(<$type>::MAX);
                        }
                        // Store max to the side metadata
                        spec.store_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);

                        let old_val = spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);

                        let old_val_from_fetch = spec.fetch_sub_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);
                        assert_eq!(old_val_from_fetch, old_val);

                        let new_val = spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        assert_eq!(new_val, 0);
                    });
                }

                #[test]
                fn [<$tname _fetch_sub_overflow>]() {
                    test_side_metadata($log_bits, |spec, data_addr, meta_addr| {
                        let max_value: $type = max_value($log_bits) as _;
                        // Set the metadata byte(s) to all 1s
                        // SAFETY: `meta_addr` is page-aligned, mapped read-write, and written
                        // sequentially inside single-threaded `serial_test`.
                        unsafe {
                            meta_addr.store::<$type>(<$type>::MAX);
                        }
                        // Store 0 to the side metadata
                        spec.store_atomic::<$type>(data_addr, 0, Ordering::SeqCst);

                        let old_val = spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);

                        // sub 1 from 0 will cause overflow, and wrap around to max
                        let old_val_from_fetch = spec.fetch_sub_atomic::<$type>(data_addr, 1, Ordering::SeqCst);
                        assert_eq!(old_val_from_fetch, old_val);

                        let new_val = spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        assert_eq!(new_val, max_value);
                    });
                }

                #[test]
                fn [<$tname _fetch_and>]() {
                    test_side_metadata($log_bits, |spec, data_addr, meta_addr| {
                        let max_value: $type = max_value($log_bits) as _;
                        // Set the metadata byte(s) to all 1s
                        // SAFETY: `meta_addr` is page-aligned, mapped read-write, and written
                        // sequentially inside single-threaded `serial_test`.
                        unsafe {
                            meta_addr.store::<$type>(<$type>::MAX);
                        }
                        // Store all 1s to the side metadata
                        spec.store_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);

                        // max and max should be max
                        let old_val = spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        let old_val_from_fetch = spec.fetch_and_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);
                        assert_eq!(old_val_from_fetch, old_val, "old values do not match");
                        assert_eq!(spec.load_atomic::<$type>(data_addr, Ordering::SeqCst), max_value, "load values do not match");
                        // SAFETY: `meta_addr` is page-aligned, mapped, initialized, and read
                        // sequentially inside single-threaded `serial_test`.
                        assert_eq!(unsafe { meta_addr.load::<$type>() }, <$type>::MAX, "raw values do not match");

                        // max and last_bit_zero should last_bit_zero
                        let last_bit_zero = max_value - 1;
                        let old_val = spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        let old_val_from_fetch = spec.fetch_and_atomic::<$type>(data_addr, last_bit_zero, Ordering::SeqCst);
                        assert_eq!(old_val_from_fetch, old_val);
                        assert_eq!(spec.load_atomic::<$type>(data_addr, Ordering::SeqCst), last_bit_zero);
                        // SAFETY: `meta_addr` is page-aligned, mapped, initialized, and read
                        // sequentially inside single-threaded `serial_test`.
                        assert_eq!(unsafe { meta_addr.load::<$type>() }, <$type>::MAX - 1);
                    });
                }

                #[test]
                fn [<$tname _fetch_or>]() {
                    test_side_metadata($log_bits, |spec, data_addr, meta_addr| {
                        let max_value: $type = max_value($log_bits) as _;
                        // Set the metadata byte(s) to all 0s
                        // SAFETY: `meta_addr` is page-aligned, mapped read-write, and written
                        // sequentially inside single-threaded `serial_test`.
                        unsafe {
                            meta_addr.store::<$type>(0);
                        }
                        // Store 0 to the side metadata
                        spec.store_atomic::<$type>(data_addr, 0, Ordering::SeqCst);

                        // 0 or 0 should be 0
                        let old_val = spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        let old_val_from_fetch = spec.fetch_or_atomic::<$type>(data_addr, 0, Ordering::SeqCst);
                        assert_eq!(old_val_from_fetch, old_val);
                        assert_eq!(spec.load_atomic::<$type>(data_addr, Ordering::SeqCst), 0);
                        // SAFETY: `meta_addr` is page-aligned, mapped, initialized, and read
                        // sequentially inside single-threaded `serial_test`.
                        assert_eq!(unsafe { meta_addr.load::<$type>() }, 0);

                        // 0 and max should max
                        let old_val = spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        let old_val_from_fetch = spec.fetch_or_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);
                        assert_eq!(old_val_from_fetch, old_val);
                        assert_eq!(spec.load_atomic::<$type>(data_addr, Ordering::SeqCst), max_value);
                        // SAFETY: `meta_addr` is page-aligned, mapped, initialized, and read
                        // sequentially inside single-threaded `serial_test`.
                        assert_eq!(unsafe { meta_addr.load::<$type>() }, max_value);
                    });
                }

                #[test]
                fn [<$tname _fetch_update_success>]() {
                    test_side_metadata($log_bits, |spec, data_addr, meta_addr| {
                        let max_value: $type = max_value($log_bits) as _;
                        // Set the metadata byte(s) to all 1s
                        // SAFETY: `meta_addr` is page-aligned, mapped read-write, and written
                        // sequentially inside single-threaded `serial_test`.
                        unsafe {
                            meta_addr.store::<$type>(<$type>::MAX);
                        }
                        // Store all 1s to the side metadata
                        spec.store_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);

                        // update from max to zero
                        let old_val = spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        let fetch_res = spec.fetch_update_atomic::<$type, _>(data_addr, Ordering::SeqCst, Ordering::SeqCst, |_x: $type| Some(0));
                        assert!(fetch_res.is_ok());
                        assert_eq!(fetch_res.unwrap(), old_val);
                        assert_eq!(spec.load_atomic::<$type>(data_addr, Ordering::SeqCst), 0);
                        // Only the affected bits are set to 0
                        // SAFETY: `meta_addr` is page-aligned, mapped, initialized, and read
                        // sequentially inside single-threaded `serial_test`.
                        assert_eq!(unsafe { meta_addr.load::<$type>() }, <$type>::MAX & (!max_value));
                    });
                }

                #[test]
                fn [<$tname _fetch_update_fail>]() {
                    test_side_metadata($log_bits, |spec, data_addr, meta_addr| {
                        let max_value: $type = max_value($log_bits) as _;
                        // Set the metadata byte(s) to all 1s
                        // SAFETY: `meta_addr` is page-aligned, mapped read-write, and written
                        // sequentially inside single-threaded `serial_test`.
                        unsafe {
                            meta_addr.store::<$type>(<$type>::MAX);
                        }
                        // Store all 1s to the side metadata
                        spec.store_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);

                        // update from max to zero
                        let old_val = spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        let fetch_res = spec.fetch_update_atomic::<$type, _>(data_addr, Ordering::SeqCst, Ordering::SeqCst, |_x: $type| None);
                        assert!(fetch_res.is_err());
                        assert_eq!(fetch_res.err().unwrap(), old_val);
                        assert_eq!(spec.load_atomic::<$type>(data_addr, Ordering::SeqCst), max_value);
                        // Only the affected bits are set to 0
                        // SAFETY: `meta_addr` is page-aligned, mapped, initialized, and read
                        // sequentially inside single-threaded `serial_test`.
                        assert_eq!(unsafe { meta_addr.load::<$type>() }, <$type>::MAX);
                    });
                }

                #[test]
                fn [<$tname _find_prev_non_zero_value_easy>]() {
                    test_side_metadata($log_bits, |spec, data_addr, _meta_addr| {
                        let max_value: $type = max_value($log_bits) as _;
                        // Store non zero value at data_addr
                        spec.store_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);

                        // Find the value starting from data_addr, at max 8 bytes.
                        // We should find data_addr
                        // SAFETY: Mapped pages in `spec` are zero-initialized by `test_side_metadata`
                        // (`$type`-aligned) and quiescent inside single-threaded `serial_test`.
                        let res_addr = unsafe { spec.find_prev_non_zero_value::<$type>(data_addr, 8) };
                        assert!(res_addr.is_some());
                        assert_eq!(res_addr.unwrap(), data_addr);
                    });
                }

                #[test]
                fn [<$tname _find_prev_non_zero_value_arbitrary_bytes>]() {
                    test_side_metadata($log_bits, |spec, data_addr, _meta_addr| {
                        let max_value: $type = max_value($log_bits) as _;
                        // Store non zero value at data_addr
                        spec.store_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);

                        // Start from data_addr, we offset arbitrary length, and search back to find data_addr
                        let test_region = (1 << TEST_LOG_BYTES_IN_REGION);
                        for len in 1..(test_region*4) {
                            let start_addr = data_addr + len;
                            // Use len+1, as len is non inclusive.
                            // SAFETY: Mapped pages in `spec` are initialized (`$type`-aligned) and
                            // quiescent inside single-threaded `serial_test`.
                            let res_addr = unsafe { spec.find_prev_non_zero_value::<$type>(start_addr, len + 1) };
                            assert!(res_addr.is_some());
                            assert_eq!(res_addr.unwrap(), data_addr);
                        }
                    });
                }

                #[test]
                fn [<$tname _find_prev_non_zero_value_arbitrary_start>]() {
                    test_side_metadata($log_bits, |spec, data_addr, _meta_addr| {
                        let max_value: $type = max_value($log_bits) as _;

                        // data_addr has a non-aligned offset
                        for offset in 0..7usize {
                            // Apply offset and test with the new data addr
                            let test_data_addr = data_addr + offset;
                            spec.store_atomic::<$type>(test_data_addr, max_value, Ordering::SeqCst);

                            // The return result should be aligned
                            // SAFETY: Mapped pages in `spec` are initialized (`$type`-aligned) and
                            // quiescent inside single-threaded `serial_test`.
                            let res_addr = unsafe { spec.find_prev_non_zero_value::<$type>(test_data_addr, 4096) };
                            assert!(res_addr.is_some());
                            assert_eq!(res_addr.unwrap(), data_addr);

                            // Clear whatever is set
                            spec.store_atomic::<$type>(test_data_addr, 0, Ordering::SeqCst);
                        }
                    });
                }

                #[test]
                fn [<$tname _find_prev_non_zero_value_no_find>]() {
                    test_side_metadata($log_bits, |spec, data_addr, _meta_addr| {
                        // Store zero value at data_addr -- so we won't find anything
                        spec.store_atomic::<$type>(data_addr, 0, Ordering::SeqCst);

                        // Start from data_addr, we offset arbitrary length, and search back
                        let test_region = (1 << TEST_LOG_BYTES_IN_REGION);
                        for len in 1..(test_region*4) {
                            let start_addr = data_addr + len;
                            // Use len+1, as len is non inclusive.
                            // SAFETY: Mapped pages in `spec` are initialized (`$type`-aligned) and
                            // quiescent inside single-threaded `serial_test`.
                            let res_addr = unsafe { spec.find_prev_non_zero_value::<$type>(start_addr, len + 1) };
                            assert!(res_addr.is_none());
                        }
                    });
                }

                #[test]
                fn [<$tname _find_next_non_zero_value_easy>]() {
                    test_side_metadata($log_bits, |spec, data_addr, _meta_addr| {
                        let max_value: $type = max_value($log_bits) as _;
                        // Store non zero value at data_addr
                        spec.store_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);

                        // Find the value starting from data_addr, at max 8 bytes.
                        // We should find data_addr
                        // SAFETY: Mapped pages in `spec` are initialized (`$type`-aligned) and
                        // quiescent inside single-threaded `serial_test`.
                        let res_addr = unsafe { spec.find_next_non_zero_value::<$type>(data_addr, 8) };
                        assert!(res_addr.is_some());
                        assert_eq!(res_addr.unwrap(), data_addr);
                    });
                }

                #[test]
                fn [<$tname _find_next_non_zero_value_arbitrary_bytes>]() {
                    test_side_metadata($log_bits, |spec, data_addr, _meta_addr| {
                        let max_value: $type = max_value($log_bits) as _;
                        let test_region = (1 << TEST_LOG_BYTES_IN_REGION);

                        // Take a data address in the middle since metadata before
                        // the start may not be mapped
                        let data_addr = data_addr + test_region*4;

                        // Store non zero value at data_addr
                        spec.store_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);
                        assert_eq!(spec.load_atomic::<$type>(data_addr, Ordering::SeqCst), max_value);

                        // Start from data_addr, we offset arbitrary length, and search forwards to find data_addr
                        for len in 1..(test_region*4) {
                            let start_addr = data_addr - len;
                            // Use len+1, as len is non inclusive.
                            // SAFETY: Mapped pages in `spec` are initialized (`$type`-aligned) and
                            // quiescent inside single-threaded `serial_test`.
                            let res_addr = unsafe { spec.find_next_non_zero_value::<$type>(start_addr, len + 1) };
                            assert!(res_addr.is_some());
                            assert_eq!(res_addr.unwrap(), data_addr);
                        }
                    });
                }

                #[test]
                fn [<$tname _find_next_non_zero_value_arbitrary_start>]() {
                    test_side_metadata($log_bits, |spec, data_addr, _meta_addr| {
                        let max_value: $type = max_value($log_bits) as _;

                        // data_addr has a non-aligned offset
                        for offset in 0..7usize {
                            // Apply offset and test with the new data addr
                            let test_data_addr = data_addr + offset;
                            spec.store_atomic::<$type>(test_data_addr, max_value, Ordering::SeqCst);

                            // The return result should be aligned
                            // SAFETY: Mapped pages in `spec` are initialized (`$type`-aligned) and
                            // quiescent inside single-threaded `serial_test`.
                            let res_addr = unsafe { spec.find_next_non_zero_value::<$type>(test_data_addr, 4096) };
                            assert!(res_addr.is_some());
                            assert_eq!(res_addr.unwrap(), data_addr);

                            // Clear whatever is set
                            spec.store_atomic::<$type>(test_data_addr, 0, Ordering::SeqCst);
                        }
                    });
                }

                #[test]
                fn [<$tname _find_next_non_zero_value_no_find>]() {
                    test_side_metadata($log_bits, |spec, data_addr, _meta_addr| {
                        // Store zero value at data_addr -- so we won't find anything
                        spec.store_atomic::<$type>(data_addr, 0, Ordering::SeqCst);

                        // Start from data_addr, we offset arbitrary length, and search back
                        let test_region = (1 << TEST_LOG_BYTES_IN_REGION);
                        for len in 1..(test_region*4) {
                            let start_addr = data_addr - len;
                            // Use len+1, as len is non inclusive.
                            // SAFETY: Mapped pages in `spec` are initialized (`$type`-aligned) and
                            // quiescent inside single-threaded `serial_test`.
                            let res_addr = unsafe { spec.find_next_non_zero_value::<$type>(start_addr, len + 1) };
                            assert!(res_addr.is_none());
                        }
                    });
                }
            }
        }
    }

    test_side_metadata_access!(test_u1, u8, 0);
    test_side_metadata_access!(test_u2, u8, 1);
    test_side_metadata_access!(test_u4, u8, 2);
    test_side_metadata_access!(test_u8, u8, 3);
    test_side_metadata_access!(test_u16, u16, 4);
    test_side_metadata_access!(test_u32, u32, 5);
    test_side_metadata_access!(test_u64, u64, 6);
    test_side_metadata_access!(
        test_usize,
        usize,
        if cfg!(target_pointer_width = "64") {
            6
        } else if cfg!(target_pointer_width = "32") {
            5
        } else {
            unreachable!()
        }
    );

    #[test]
    fn test_bulk_update_meta_bits() {
        let mut raw_word: u64 = 0;
        let with_addr = |raw: &mut u64, f: fn(Address)| {
            let addr = Address::from_mut_ptr(raw);
            f(addr);
        };

        with_addr(&mut raw_word, |addr| {
            SideMetadataSpec::set_meta_bits(addr, 0, addr, 4);
        });
        assert_eq!(u64::from_le(raw_word), 0b1111);

        with_addr(&mut raw_word, |addr| {
            SideMetadataSpec::zero_meta_bits(addr, 1, addr, 3);
        });
        assert_eq!(u64::from_le(raw_word), 0b1001);

        with_addr(&mut raw_word, |addr| {
            SideMetadataSpec::set_meta_bits(addr, 2, addr, 6);
        });
        assert_eq!(u64::from_le(raw_word), 0b0011_1101);

        with_addr(&mut raw_word, |addr| {
            SideMetadataSpec::zero_meta_bits(addr, 0, addr + 1usize, 0);
        });
        assert_eq!(u64::from_le(raw_word), 0b0);

        with_addr(&mut raw_word, |addr| {
            SideMetadataSpec::set_meta_bits(addr, 2, addr + 1usize, 2);
        });
        assert_eq!(u64::from_le(raw_word), 0b11_1111_1100);

        with_addr(&mut raw_word, |addr| {
            SideMetadataSpec::set_meta_bits(addr, 0, addr + 1usize, 2);
        });
        assert_eq!(u64::from_le(raw_word), 0b11_1111_1111);
    }

    #[test]
    #[should_panic(expected = "SideMetadataSpec offset")]
    fn test_misaligned_u64_spec_offset_panics() {
        let spec = SideMetadataSpec {
            name: "misaligned_u64",
            is_global: true,
            offset: 4,
            log_num_of_bits: 6,
            log_bytes_in_region: 3,
        };
        spec.assert_value_type::<u64>(None);
    }

    #[test]
    fn test_release_acqrel_orderings_on_subbyte_spec() {
        test_side_metadata(1, |spec, data_addr, _meta_addr| {
            spec.store_atomic::<u8>(data_addr, 0b10, Ordering::Release);
            assert_eq!(spec.load_atomic::<u8>(data_addr, Ordering::Acquire), 0b10);

            assert_eq!(
                spec.compare_exchange_atomic::<u8>(
                    data_addr,
                    0b10,
                    0b01,
                    Ordering::Release,
                    Ordering::Relaxed
                ),
                Ok(0b10)
            );
            assert_eq!(
                spec.compare_exchange_atomic::<u8>(
                    data_addr,
                    0b01,
                    0b10,
                    Ordering::AcqRel,
                    Ordering::Acquire
                ),
                Ok(0b01)
            );

            assert_eq!(
                spec.fetch_add_atomic::<u8>(data_addr, 1, Ordering::Release),
                0b10
            );
            assert_eq!(
                spec.fetch_add_atomic::<u8>(data_addr, 1, Ordering::AcqRel),
                0b11
            );
            assert_eq!(
                spec.fetch_sub_atomic::<u8>(data_addr, 1, Ordering::Release),
                0b00
            );
            assert_eq!(
                spec.fetch_sub_atomic::<u8>(data_addr, 1, Ordering::AcqRel),
                0b11
            );
            assert_eq!(spec.load_atomic::<u8>(data_addr, Ordering::Acquire), 0b10);
        });
    }

    #[test]
    fn test_mapped_slice_helpers() {
        test_side_metadata(3, |spec, data_addr, _meta_addr| {
            // `TEST_LOG_BYTES_IN_REGION == 12` (4 KiB/region), `log_bits == 3` (1 byte/region).
            // `test_side_metadata` maps 1 region (`4 KiB` => `1` metadata byte); map a full 4-page
            // (`16 KiB` => `4` metadata bytes) window for slice/chunk projections.
            let four_pages = 4 << TEST_LOG_BYTES_IN_REGION;
            let ctx = SideMetadataContext {
                global: vec![*spec],
                local: vec![],
            };
            ctx.try_map_metadata_space(data_addr, four_pages, "SliceHelperTest")
                .unwrap();

            {
                // SAFETY: `spec` is mapped for `[data_addr, data_addr + four_pages)`, single-threaded,
                // and exclusively borrowed within this inner block.
                let bytes_mut = unsafe { spec.mapped_bytes_mut_unchecked(data_addr, four_pages) };
                assert_eq!(bytes_mut.len(), 4);
                bytes_mut.copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);
            }

            {
                // SAFETY: `bytes_mut` was dropped above; `[data_addr, data_addr + four_pages)` is
                // mapped, initialized, and immutable for the duration of this block.
                let bytes = unsafe { spec.mapped_bytes_unchecked(data_addr, four_pages) };
                assert_eq!(bytes, &[0x11, 0x22, 0x33, 0x44]);
            }

            {
                // SAFETY: Previous non-atomic slices were dropped; only `AtomicU8` operations touch
                // the 4 metadata bytes inside this block.
                let atomics = unsafe { spec.mapped_atomic_bytes_unchecked::<4>(data_addr) };
                assert_eq!(atomics[0].load(Ordering::Relaxed), 0x11);
                atomics[2].store(0x99, Ordering::Relaxed);

                // SAFETY: Coexisting `&[[AtomicU8; 2]; 2]` shares only `AtomicU8` references.
                let grid = unsafe { spec.mapped_atomic_byte_chunks_unchecked::<2, 2>(data_addr) };
                assert_eq!(grid[1][0].load(Ordering::Relaxed), 0x99);
                assert_eq!(grid[1][1].load(Ordering::Relaxed), 0x44);
            }
        });
    }

    #[test]
    #[should_panic(expected = "byte boundary")]
    fn test_mapped_bytes_unaligned_subbyte_panics() {
        serial_test(|| {
            core_test_initialize_side_metadata();
            let spec = SideMetadataSpec {
                name: "subbyte_slice_test",
                is_global: true,
                offset: 0,
                log_num_of_bits: 0, // 1 bit per 8-byte word => 64 bytes per metadata byte
                log_bytes_in_region: 3,
            };
            let unaligned_data = vm_layout::vm_layout().heap_start + 8usize;
            // SAFETY: Expected to panic in `checked_mapped_byte_range` before any pointer dereference.
            let _ = unsafe { spec.mapped_bytes_unchecked(unaligned_data, 64) };
        });
    }

    #[test]
    #[should_panic(expected = "overflows usize")]
    fn test_mapped_atomic_bytes_shift_overflow_panics() {
        let spec = SideMetadataSpec {
            name: "shift_overflow_slice_test",
            is_global: true,
            offset: 0,
            log_num_of_bits: 0,
            log_bytes_in_region: (usize::BITS as usize) - 2,
        };
        // SAFETY: Expected to panic in `checked_meta_to_data_bytes` before any pointer dereference.
        let _ = unsafe { spec.mapped_atomic_bytes_unchecked::<1>(Address::ZERO) };
    }
}
