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
    ) -> AtomicMetaRef<'a, T> {
        self.assert_spec::<T>();
        let meta_addr = address_to_meta_address(self, data_addr);
        // SAFETY: `assert_spec::<T>()` verified width (`T::LOG2 == 3` for sub-byte, or
        // `T::LOG2 == self.log_num_of_bits` with `self.offset % max_metadata_align::<T>() == 0`),
        // `assert_atomic_align` unconditionally checks non-null and `ATOMIC_ALIGN`, and caller
        // guarantees `meta_addr` is mapped, initialized, and accessed solely via the corresponding
        // atomic width for `'a`.
        unsafe {
            if self.log_num_of_bits < 3 {
                assert_atomic_align::<u8>(meta_addr);
                let shift = meta_byte_lshift(self, data_addr);
                let mask = meta_byte_mask(self) << shift;
                AtomicMetaRef::SubByte {
                    byte: meta_addr.as_ref::<AtomicU8>(),
                    shift,
                    mask,
                }
            } else {
                assert_atomic_align::<T>(meta_addr);
                AtomicMetaRef::Full(meta_addr.as_ref::<T::Atomic>())
            }
        }
    }

    /// Validate `self`'s `<= 8`-bit geometry, byte-boundary alignment of `data_addr`, single-chunk
    /// containment (when chunked), startup reservation + spec-window bounds, and `MMAPPER` mapping
    /// of `[meta_addr, meta_addr + meta_bytes)`, returning `meta_addr`.
    #[inline]
    fn validate_mapped_atomic_bytes(&self, data_addr: Address, meta_bytes: usize) -> Address {
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
        let region_bytes = 1usize << self.log_bytes_in_region;
        assert!(
            !data_addr.is_zero() && data_addr.is_aligned_to(region_bytes),
            "data_addr ({}) must be non-zero and aligned to region_bytes (0x{:x}) for {:?}",
            data_addr,
            region_bytes,
            self
        );
        assert_eq!(
            meta_byte_lshift(self, data_addr),
            0,
            "data_addr ({}) does not start on a side-metadata byte boundary for {:?}",
            data_addr,
            self
        );
        assert!(
            meta_bytes <= isize::MAX as usize,
            "SideMetadataSpec slice byte length (0x{:x}) exceeds isize::MAX",
            meta_bytes
        );
        if meta_bytes > 0 {
            let bytes_per_meta_byte = 1usize << shift;
            let last_data_offset = (meta_bytes - 1)
                .checked_mul(bytes_per_meta_byte)
                .and_then(|off| off.checked_add(bytes_per_meta_byte - 1))
                .expect("SideMetadataSpec slice data range overflow");
            let last_raw = data_addr
                .as_usize()
                .checked_add(last_data_offset)
                .expect("SideMetadataSpec slice data range overflow");
            if self.uses_chunked_side_metadata() {
                let last_data_byte = data_addr + last_data_offset;
                assert_eq!(
                    data_addr.align_down(BYTES_IN_CHUNK),
                    last_data_byte.align_down(BYTES_IN_CHUNK),
                    "Chunked side-metadata slice for {:?} cannot span multiple chunks",
                    self
                );
            } else {
                assert!(
                    last_raw <= helpers::CONTIGUOUS_DATA_MAX,
                    "Contiguous side-metadata slice data range overflowed architectural address space: start={}, last={:#x}, spec={:?}",
                    data_addr,
                    last_raw,
                    self
                );
            }
        }
        let meta_addr = address_to_meta_address(self, data_addr);
        assert!(
            meta_addr.as_usize().checked_add(meta_bytes).is_some(),
            "SideMetadataSpec slice metadata address range overflow ({} + 0x{:x})",
            meta_addr,
            meta_bytes
        );
        let meta_end = meta_addr + meta_bytes;
        helpers::assert_meta_range_in_spec_bounds(self, data_addr, meta_addr, meta_end);
        if meta_bytes > 0 {
            let grain = MMAPPER.granularity();
            let first_grain = meta_addr.align_down(grain);
            let last_grain = (meta_end - 1usize).align_down(grain);
            let mut cur_grain = first_grain;
            loop {
                assert!(
                    MMAPPER.is_mapped_address(cur_grain),
                    "Unmapped side-metadata grain {} in [{}, {}) for {:?} (data_addr={})",
                    cur_grain,
                    meta_addr,
                    meta_end,
                    self,
                    data_addr
                );
                if cur_grain == last_grain {
                    break;
                }
                cur_grain += grain;
            }
        }
        meta_addr
    }

    /// Project the contiguous `N`-byte side-metadata range starting at `data_addr` as
    /// `&'a [AtomicU8; N]`.
    ///
    /// Runtime assertions unconditionally verify `self`'s `<= 8`-bit geometry, `N <= isize::MAX`,
    /// byte-boundary alignment, single-chunk containment (when chunked), side-metadata reservation
    /// and spec-window bounds, and `MMAPPER` mapping across `[meta_addr, meta_addr + N)`.
    ///
    /// # Safety
    /// * The mapped side-metadata bytes `[meta_addr, meta_addr + N)` must be initialized and
    ///   remain mapped for the entire lifetime `'a`.
    /// * For `'a`, all accesses to `[meta_addr, meta_addr + N)` across all threads must use
    ///   `AtomicU8` exclusively (no non-atomic accesses, mixed-width atomics, or `&[u8]` /
    ///   `&mut [u8]` aliases).
    #[inline(always)]
    pub(crate) unsafe fn mapped_atomic_bytes_unchecked<'a, const N: usize>(
        &self,
        data_addr: Address,
    ) -> &'a [AtomicU8; N] {
        const {
            assert!(N <= isize::MAX as usize);
        }
        let meta_addr = self.validate_mapped_atomic_bytes(data_addr, N);
        // SAFETY: `validate_mapped_atomic_bytes` verified non-null `meta_addr`, `N <= isize::MAX`,
        // spec-window bounds, and `MMAPPER` mapping across `[meta_addr, meta_addr + N)` (`AtomicU8`
        // has alignment 1); caller guarantees initialization, lifetime `'a` mapping stability, and
        // `AtomicU8`-only access without non-atomic aliases.
        unsafe { &*meta_addr.to_ptr::<[AtomicU8; N]>() }
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
    /// and invokes the given method to update the metadata bits.
    pub(super) fn bulk_update_metadata(
        &self,
        start: Address,
        size: usize,
        update_meta_bits: &impl Fn(Address, u8, Address, u8),
    ) {
        if self.uses_contiguous_side_metadata() {
            if let Some((ms, msb, me, meb)) =
                helpers::half_open_contiguous_meta_bit_range(self, start, size)
            {
                update_meta_bits(ms, msb, me, meb);
            }
        } else {
            // On 32-bit targets, local policy-specific side metadata is chunked: adjacent data chunks
            // do not have contiguous metadata, so split across `BYTES_IN_CHUNK` boundaries and compute
            // each sub-chunk's end bit offset directly from `data_bytes`.
            #[cfg(target_pointer_width = "32")]
            helpers::for_each_spec_data_span(self, start, size, |data_start, data_bytes| {
                use crate::util::constants::BITS_IN_BYTE;
                debug_assert_eq!(
                    data_start.align_down(BYTES_IN_CHUNK),
                    (data_start + (data_bytes - 1)).align_down(BYTES_IN_CHUNK),
                    "The data to be updated in discontiguous specs needs to be in the same chunk"
                );
                let meta_start = address_to_meta_address(self, data_start);
                let meta_start_shift = meta_byte_lshift(self, data_start);
                let meta_total_bits =
                    (data_bytes >> self.log_bytes_in_region) << self.log_num_of_bits;
                let mut meta_end = meta_start + (meta_total_bits >> LOG_BITS_IN_BYTE);
                let mut meta_end_shift =
                    meta_start_shift + ((meta_total_bits % BITS_IN_BYTE) as u8);
                if meta_end_shift >= BITS_IN_BYTE as u8 {
                    meta_end_shift -= BITS_IN_BYTE as u8;
                    meta_end += 1usize;
                }
                update_meta_bits(meta_start, meta_start_shift, meta_end, meta_end_shift);
            });
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

        let Some((dst_meta_start_addr, dst_meta_start_bit, dst_meta_end_addr, dst_meta_end_bit)) =
            helpers::half_open_contiguous_meta_bit_range(self, start, size)
        else {
            return;
        };

        let src_meta_start_addr = address_to_meta_address(other, start);
        debug_assert_eq!(dst_meta_start_bit, meta_byte_lshift(other, start));

        let mut visitor = |range| {
            // SAFETY: Both `self` and `other` are verified contiguous specs with matching region
            // and bit widths, covering mapped, initialized, non-null ranges that are disjoint (or
            // identical when `self == other`). Full interior bytes (`BitByteRange::Bytes`) are
            // quiescent (`u8` alignment 1), while shared boundary bytes (`BitsInByte`) use
            // `AtomicU8` (`load_atomic` + `fetch_update`) to preserve `!mask` bits atomically.
            unsafe {
                match range {
                    BitByteRange::Bytes {
                        start: dst_start,
                        end: dst_end,
                    } => {
                        let src_start = src_meta_start_addr + (dst_start - dst_meta_start_addr);
                        std::ptr::copy::<u8>(
                            src_start.to_ptr(),
                            dst_start.to_mut_ptr(),
                            dst_end - dst_start,
                        );
                    }
                    BitByteRange::BitsInByte {
                        addr: dst,
                        bit_start,
                        bit_end,
                    } => {
                        let src = src_meta_start_addr + (dst - dst_meta_start_addr);
                        let mask: u8 = !(u8::MAX.checked_shl(bit_end as u32).unwrap_or(0))
                            & (u8::MAX << bit_start);
                        let old_src = <u8 as MetadataValue>::load_atomic(src, Ordering::Relaxed);
                        let _ = <u8 as MetadataValue>::fetch_update(
                            dst,
                            Ordering::Relaxed,
                            Ordering::Relaxed,
                            |old_dst| Some((old_src & mask) | (old_dst & !mask)),
                        );
                    }
                }
            }
            false
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
                // SAFETY: `assert_value_type` verified `T` width and offset alignment; caller
                // guarantees `meta_addr` is non-null, mapped, initialized, aligned for `T`, and
                // free of concurrent writes, atomic modifications, or live `&mut` aliases.
                unsafe {
                    if self.log_num_of_bits < 3 {
                        let lshift = meta_byte_lshift(self, data_addr);
                        let mask = meta_byte_mask(self) << lshift;
                        let byte_val = <u8 as MetadataValue>::load(meta_addr);
                        FromPrimitive::from_u8((byte_val & mask) >> lshift).unwrap()
                    } else {
                        <T as MetadataValue>::load(meta_addr)
                    }
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
                // SAFETY: `assert_value_type` verified `T` width and offset alignment; caller
                // guarantees `meta_addr` is non-null, mapped, initialized, writable, aligned for
                // `T`, and exclusively accessed without concurrent accesses or aliasing references.
                unsafe {
                    if self.log_num_of_bits < 3 {
                        let lshift = meta_byte_lshift(self, data_addr);
                        let mask = meta_byte_mask(self) << lshift;
                        let old_val = <u8 as MetadataValue>::load(meta_addr);
                        let new_val = (old_val & !mask) | (metadata.to_u8().unwrap() << lshift);
                        <u8 as MetadataValue>::store(meta_addr, new_val);
                    } else {
                        <T as MetadataValue>::store(meta_addr, metadata);
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
        // SAFETY: Caller guarantees `meta_addr` is non-null, mapped, initialized (`u8` alignment 1),
        // and free of concurrent writes, atomic modifications, or live `&mut` aliases.
        unsafe { <u8 as MetadataValue>::load(meta_addr) }
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
        // SAFETY: Caller guarantees `meta_addr` is non-null, mapped, writable (`u8` alignment 1),
        // and exclusively accessed without concurrent accesses or aliasing references.
        unsafe {
            <u8 as MetadataValue>::store(meta_addr, byte);
        }
    }

    /// Loads a value from the side metadata for the given address.
    /// This method has similar semantics to `load` in Rust atomics.
    pub fn load_atomic<T: MetadataValue>(&self, data_addr: Address, order: Ordering) -> T {
        self.side_metadata_access::<true, T, _, _, _>(
            data_addr,
            None,
            || {
                // SAFETY: `assert_value_type` verified `T` and alignment; `SideMetadataSpec`'s
                // mapped-region contract guarantees an initialized, atomic-only metadata cell.
                unsafe { self.resolve_atomic::<T>(data_addr) }.load(None, order)
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
                // SAFETY: `assert_value_type` verified `T` and alignment; `SideMetadataSpec`'s
                // mapped-region contract guarantees an initialized, atomic-only metadata cell.
                unsafe { self.resolve_atomic::<T>(data_addr) }.store(metadata, None, order);
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
        // SAFETY: Caller upholds `SideMetadataSpec::store` exclusive-access and mapped invariants
        // for `data_addr`, and each arm dispatches to the matching `T` for `self.log_num_of_bits`.
        unsafe {
            match self.log_num_of_bits {
                0..=3 => self.store(data_addr, u8::zero()),
                4 => self.store(data_addr, u16::zero()),
                5 => self.store(data_addr, u32::zero()),
                6 => self.store(data_addr, u64::zero()),
                _ => unreachable!(),
            }
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
                // SAFETY: `assert_value_type` verified `T` and alignment; `SideMetadataSpec`'s
                // mapped-region contract guarantees an initialized, atomic-only metadata cell.
                unsafe { self.resolve_atomic::<T>(data_addr) }.compare_exchange(
                    old_metadata,
                    new_metadata,
                    None,
                    success_order,
                    failure_order,
                )
            },
            |_res| {
                #[cfg(feature = "extreme_assertions")]
                if _res.is_ok() {
                    sanity::verify_store(self, data_addr, new_metadata);
                }
            },
        )
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
                // SAFETY: `assert_value_type` verified `T` and alignment; `SideMetadataSpec`'s
                // mapped-region contract guarantees an initialized, atomic-only metadata cell.
                unsafe { self.resolve_atomic::<T>(data_addr) }.fetch_add(val, order)
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
                // SAFETY: `assert_value_type` verified `T` and alignment; `SideMetadataSpec`'s
                // mapped-region contract guarantees an initialized, atomic-only metadata cell.
                unsafe { self.resolve_atomic::<T>(data_addr) }.fetch_sub(val, order)
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
                // SAFETY: `assert_value_type` verified `T` and alignment; `SideMetadataSpec`'s
                // mapped-region contract guarantees an initialized, atomic-only metadata cell.
                unsafe { self.resolve_atomic::<T>(data_addr) }.fetch_and(val, order)
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
                // SAFETY: `assert_value_type` verified `T` and alignment; `SideMetadataSpec`'s
                // mapped-region contract guarantees an initialized, atomic-only metadata cell.
                unsafe { self.resolve_atomic::<T>(data_addr) }.fetch_or(val, order)
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
        #[cfg(feature = "extreme_assertions")]
        let last_new_val: std::cell::Cell<Option<T>> = std::cell::Cell::new(None);
        #[cfg(feature = "extreme_assertions")]
        let last_new_val_ref = &last_new_val;
        self.side_metadata_access::<true, T, _, _, _>(
            data_addr,
            None,
            move || -> std::result::Result<T, T> {
                // SAFETY: `assert_value_type` verified `T` and alignment; `SideMetadataSpec`'s
                // mapped-region contract guarantees an initialized, atomic-only metadata cell.
                unsafe { self.resolve_atomic::<T>(data_addr) }.fetch_update(
                    set_order,
                    fetch_order,
                    |old_val| {
                        let new_val = f(old_val);
                        #[cfg(feature = "extreme_assertions")]
                        last_new_val_ref.set(new_val);
                        new_val
                    },
                )
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
    pub unsafe fn find_prev_non_zero_value<T: MetadataValue>(
        &self,
        data_addr: Address,
        search_limit_bytes: usize,
    ) -> Option<Address> {
        self.assert_value_type::<T>(None);
        if search_limit_bytes == 0 {
            return None;
        }
        let region_bytes = 1usize << self.log_bytes_in_region;
        let cur_region = data_addr.align_down(region_bytes);
        let lower_bound = data_addr.saturating_sub(search_limit_bytes - 1);
        let first_region =
            if cur_region.as_usize() >= region_bytes && lower_bound <= cur_region - region_bytes {
                lower_bound.align_up(region_bytes)
            } else {
                cur_region
            };

        let search_simple = || {
            let mmap_granularity = MMAPPER.granularity();
            let mut mapped_meta_grain: Option<Address> = None;
            let mut cursor = cur_region;
            loop {
                let meta_grain = address_to_meta_address(self, cursor).align_down(mmap_granularity);
                if mapped_meta_grain != Some(meta_grain) {
                    if MMAPPER.is_mapped_address(meta_grain) {
                        mapped_meta_grain = Some(meta_grain);
                    } else {
                        return None;
                    }
                }
                // SAFETY: `meta_grain` was verified mapped above; caller guarantees mapped grains
                // in the searched window are initialized, `T`-aligned, and free of concurrent writes.
                if !unsafe { self.load::<T>(cursor) }.is_zero() {
                    return Some(cursor);
                }
                if cursor <= first_region {
                    break;
                }
                cursor -= region_bytes;
            }
            None
        };

        if !self.uses_contiguous_side_metadata() {
            warn!("We are trying to search non zero bits in an discontiguous side metadata. The performance is slow, as MMTk does not optimize for this case.");
            return search_simple();
        }

        if data_addr.as_usize() > helpers::CONTIGUOUS_DATA_MAX {
            return None;
        }

        let result = helpers::inclusive_contiguous_meta_range(self, first_region, cur_region)
            .and_then(|(byte_start, byte_end, first_mask, last_mask)| {
                helpers::try_for_each_meta_grain_backward(
                    byte_start,
                    byte_end,
                    first_mask,
                    last_mask,
                    |g_start, g_end, g_first, g_last| {
                        if !g_start.is_mapped() {
                            return core::ops::ControlFlow::Break(None);
                        }
                        let len = g_end - g_start;
                        debug_assert!(!g_start.is_zero() && len <= isize::MAX as usize);
                        // SAFETY: `g_start.is_mapped()` verified this single `MMAPPER` grain is
                        // mapped (`u8` alignment 1), and caller guarantees mapped side metadata in
                        // the searched window is initialized and free of concurrent mutations.
                        let slice =
                            unsafe { core::slice::from_raw_parts(g_start.to_ptr::<u8>(), len) };
                        let align_rem = g_start.as_usize() & (core::mem::size_of::<usize>() - 1);
                        if let Some((byte_off, bit)) = helpers::find_last_non_zero_bit_in_slice(
                            slice, align_rem, g_first, g_last,
                        ) {
                            let (addr, bit) = align_metadata_address(self, g_start + byte_off, bit);
                            let found = contiguous_meta_address_to_address(self, addr, bit)
                                .align_down(region_bytes);
                            return core::ops::ControlFlow::Break(Some(found));
                        }
                        core::ops::ControlFlow::Continue(())
                    },
                )
                .flatten()
            });

        #[cfg(debug_assertions)]
        assert_eq!(
            result,
            search_simple(),
            "find_prev_non_zero_value fast path diverged from simple scan"
        );

        result
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
    pub unsafe fn find_next_non_zero_value<T: MetadataValue>(
        &self,
        data_addr: Address,
        search_limit_bytes: usize,
    ) -> Option<Address> {
        self.assert_value_type::<T>(None);
        if search_limit_bytes == 0 {
            return None;
        }
        let max_addr = if self.uses_contiguous_side_metadata() {
            helpers::CONTIGUOUS_DATA_MAX
        } else {
            usize::MAX
        };
        if data_addr.as_usize() > max_addr {
            return None;
        }
        let region_bytes = 1usize << self.log_bytes_in_region;
        let first_region = data_addr.align_down(region_bytes);
        let last_raw = data_addr
            .as_usize()
            .saturating_add(search_limit_bytes - 1)
            .min(max_addr);
        let last_byte = data_addr + (last_raw - data_addr.as_usize());
        let last_region = last_byte.align_down(region_bytes);

        let search_simple = || {
            let mmap_granularity = MMAPPER.granularity();
            let mut mapped_meta_grain: Option<Address> = None;
            let mut cursor = first_region;
            loop {
                let meta_grain = address_to_meta_address(self, cursor).align_down(mmap_granularity);
                if mapped_meta_grain != Some(meta_grain) {
                    if MMAPPER.is_mapped_address(meta_grain) {
                        mapped_meta_grain = Some(meta_grain);
                    } else {
                        return None;
                    }
                }
                // SAFETY: `meta_grain` was verified mapped above; caller guarantees mapped grains
                // in the searched window are initialized, `T`-aligned, and free of concurrent writes.
                if !unsafe { self.load::<T>(cursor) }.is_zero() {
                    return Some(cursor);
                }
                if cursor >= last_region {
                    break;
                }
                cursor += region_bytes;
            }
            None
        };

        if !self.uses_contiguous_side_metadata() {
            warn!("We are trying to search non zero bits in an discontiguous side metadata. The performance is slow, as MMTk does not optimize for this case.");
            return search_simple();
        }

        let result = helpers::inclusive_contiguous_meta_range(self, first_region, last_region)
            .and_then(|(byte_start, byte_end, first_mask, last_mask)| {
                helpers::try_for_each_meta_grain_forward(
                    byte_start,
                    byte_end,
                    first_mask,
                    last_mask,
                    |g_start, g_end, g_first, g_last| {
                        if !g_start.is_mapped() {
                            return core::ops::ControlFlow::Break(None);
                        }
                        let len = g_end - g_start;
                        debug_assert!(!g_start.is_zero() && len <= isize::MAX as usize);
                        // SAFETY: `g_start.is_mapped()` verified this single `MMAPPER` grain is
                        // mapped (`u8` alignment 1), and caller guarantees mapped side metadata in
                        // the searched window is initialized and free of concurrent mutations.
                        let slice =
                            unsafe { core::slice::from_raw_parts(g_start.to_ptr::<u8>(), len) };
                        let align_rem = g_start.as_usize() & (core::mem::size_of::<usize>() - 1);
                        if let Some((byte_off, bit)) = helpers::find_first_non_zero_bit_in_slice(
                            slice, align_rem, g_first, g_last,
                        ) {
                            let (addr, bit) = align_metadata_address(self, g_start + byte_off, bit);
                            let found = contiguous_meta_address_to_address(self, addr, bit)
                                .align_down(region_bytes);
                            return core::ops::ControlFlow::Break(Some(found));
                        }
                        core::ops::ControlFlow::Continue(())
                    },
                )
                .flatten()
            });

        #[cfg(debug_assertions)]
        assert_eq!(
            result,
            search_simple(),
            "find_next_non_zero_value fast path diverged from simple scan. data_addr {}, search_limit_bytes {}",
            data_addr,
            search_limit_bytes,
        );

        result
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
            let start_meta_addr = address_to_contiguous_meta_address(self, data_start_addr);
            let start_meta_shift = meta_byte_lshift(self, data_start_addr);
            let end_meta_addr = address_to_contiguous_meta_address(self, data_end_addr);
            let end_meta_shift = meta_byte_lshift(self, data_end_addr);
            let Some((byte_start, byte_end, first_mask, last_mask)) =
                helpers::active_bit_range_bytes(
                    start_meta_addr,
                    start_meta_shift,
                    end_meta_addr,
                    end_meta_shift,
                )
            else {
                return;
            };
            let len = byte_end - byte_start;
            assert!(!byte_start.is_zero() && len <= isize::MAX as usize);
            let align_rem = byte_start.as_usize() & (core::mem::size_of::<usize>() - 1);
            // SAFETY: Non-null `byte_start` and `len <= isize::MAX` were verified above (`u8` has
            // alignment 1), and caller guarantees `[data_start_addr, data_end_addr)` has fully
            // mapped, initialized, quiescent side metadata without concurrent mutations (including
            // inside `visit_data`) or live `&mut` aliases.
            let slice = unsafe { core::slice::from_raw_parts(byte_start.to_ptr::<u8>(), len) };
            helpers::scan_non_zero_bits_in_slice(
                slice,
                align_rem,
                first_mask,
                last_mask,
                |byte_off, bit| {
                    visit_data(helpers::contiguous_meta_address_to_address(
                        self,
                        byte_start + byte_off,
                        bit,
                    ));
                },
            );
        } else {
            warn!(
                "We are trying to search for non zero bits in a discontiguous side metadata \
            or the metadata has more than one bit per region. \
                The performance is slow, as MMTk does not optimize for this case."
            );
            let region_bytes = 1usize << self.log_bytes_in_region;
            let mut cursor = data_start_addr;
            while cursor < data_end_addr {
                debug_assert!(self.is_mapped(cursor));
                // SAFETY: Caller guarantees `cursor` in `[data_start_addr, data_end_addr)` has
                // mapped, initialized, `T`-aligned side metadata with no concurrent writes or
                // `&mut` aliases.
                if !unsafe { self.load::<T>(cursor) }.is_zero() {
                    visit_data(cursor);
                }
                cursor += region_bytes;
            }
        }
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

    /// Single-threaded (`!Send + !Sync`) bounds-checked side-metadata test arena backed by a
    /// mapped metadata page inside `serial_test`.
    struct TestSideMetaArena {
        spec: SideMetadataSpec,
        data_addr: Address,
        meta_addr: Address,
        meta_page_start: Address,
        meta_page_end: Address,
        _not_send_sync: core::marker::PhantomData<*mut ()>,
    }

    impl TestSideMetaArena {
        fn assert_entry_in_bounds<T: MetadataValue>(&self, addr: Address) {
            self.spec.assert_value_type::<T>(None);
            let entry = address_to_meta_address(&self.spec, addr);
            let bytes = core::mem::size_of::<T>();
            assert!(entry.is_aligned_to(core::mem::align_of::<T>()));
            assert!(
                entry >= self.meta_page_start && entry + bytes <= self.meta_page_end,
                "metadata entry {entry}..{} outside mapped test page {}..{}",
                entry + bytes,
                self.meta_page_start,
                self.meta_page_end
            );
        }

        fn load_raw<T: MetadataValue>(&self) -> T {
            self.assert_entry_in_bounds::<T>(self.data_addr);
            // SAFETY: `assert_entry_in_bounds` verified `self.meta_addr` is `T`-aligned and within
            // the mapped, zero-initialized test metadata page; `!Send + !Sync` inside `serial_test`
            // and `&self` guarantee no concurrent writes or live `&mut` aliases.
            unsafe { self.meta_addr.load::<T>() }
        }

        fn store_raw<T: MetadataValue>(&mut self, val: T) {
            self.assert_entry_in_bounds::<T>(self.data_addr);
            // SAFETY: `assert_entry_in_bounds` verified `self.meta_addr` is `T`-aligned and within
            // the mapped writable test metadata page; `&mut self` inside single-threaded
            // `serial_test` guarantees exclusive access.
            unsafe { self.meta_addr.store::<T>(val) }
        }

        fn load<T: MetadataValue>(&self, addr: Address) -> T {
            self.assert_entry_in_bounds::<T>(addr);
            // SAFETY: `assert_entry_in_bounds` verified `addr`'s metadata cell is mapped,
            // initialized, and `T`-aligned; `&self` inside `serial_test` guarantees no concurrent
            // writes or `&mut` aliases.
            unsafe { self.spec.load::<T>(addr) }
        }

        fn store<T: MetadataValue>(&mut self, addr: Address, val: T) {
            self.assert_entry_in_bounds::<T>(addr);
            // SAFETY: `assert_entry_in_bounds` verified `addr`'s metadata cell is mapped,
            // initialized, writable, and `T`-aligned; `&mut self` inside `serial_test` guarantees
            // exclusive access.
            unsafe { self.spec.store::<T>(addr, val) }
        }

        fn find_prev<T: MetadataValue>(
            &self,
            start_addr: Address,
            search_limit_bytes: usize,
        ) -> Option<Address> {
            self.spec.assert_value_type::<T>(None);
            // SAFETY: `assert_value_type` verified `T` matches `self.spec` (`T`-aligned entries),
            // all mapped grains in this single-threaded `serial_test` are initialized and unaliased
            // under `&self`, and `find_prev_non_zero_value` stops at unmapped `MMAPPER` grains.
            unsafe {
                self.spec
                    .find_prev_non_zero_value::<T>(start_addr, search_limit_bytes)
            }
        }

        fn find_next<T: MetadataValue>(
            &self,
            start_addr: Address,
            search_limit_bytes: usize,
        ) -> Option<Address> {
            self.spec.assert_value_type::<T>(None);
            // SAFETY: `assert_value_type` verified `T` matches `self.spec` (`T`-aligned entries),
            // all mapped grains in this single-threaded `serial_test` are initialized and unaliased
            // under `&self`, and `find_next_non_zero_value` stops at unmapped `MMAPPER` grains.
            unsafe {
                self.spec
                    .find_next_non_zero_value::<T>(start_addr, search_limit_bytes)
            }
        }
    }

    fn test_side_metadata(
        log_bits: usize,
        f: impl Fn(&mut TestSideMetaArena) + std::panic::RefUnwindSafe,
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
                    let meta_page_start = meta_addr.align_down(BYTES_IN_PAGE);
                    assert!(meta_page_start.is_mapped());
                    let mut arena = TestSideMetaArena {
                        spec,
                        data_addr,
                        meta_addr,
                        meta_page_start,
                        meta_page_end: meta_page_start + BYTES_IN_PAGE,
                        _not_send_sync: core::marker::PhantomData,
                    };

                    f(&mut arena);
                },
                || {
                    assert!(log_bits <= 6);
                    let meta_page_start = meta_addr.align_down(BYTES_IN_PAGE);
                    if meta_page_start.is_mapped() {
                        crate::util::memory::zero(meta_page_start, BYTES_IN_PAGE);
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
                    test_side_metadata($log_bits, |arena| {
                        let data_addr = arena.data_addr;
                        assert_eq!(arena.load::<$type>(data_addr), 0);
                        assert_eq!(arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst), 0);

                        let max_value: $type = max_value($log_bits) as _;
                        arena.store::<$type>(data_addr, max_value);
                        assert_eq!(arena.load::<$type>(data_addr), max_value);
                        assert_eq!(arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst), max_value);
                        assert_eq!(arena.load_raw::<$type>(), max_value);
                    });
                }

                #[test]
                fn [<$tname _store>]() {
                    test_side_metadata($log_bits, |arena| {
                        let data_addr = arena.data_addr;
                        let max_value: $type = max_value($log_bits) as _;

                        arena.store_raw::<$type>(<$type>::MAX);
                        arena.store::<$type>(data_addr, 0);
                        assert_eq!(arena.load::<$type>(data_addr), 0);
                        // Only the affected bits are set to 0
                        assert_eq!(arena.load_raw::<$type>(), <$type>::MAX & (!max_value));
                    });
                }

                #[test]
                fn [<$tname _atomic_store>]() {
                    test_side_metadata($log_bits, |arena| {
                        let data_addr = arena.data_addr;
                        let max_value: $type = max_value($log_bits) as _;

                        arena.store_raw::<$type>(<$type>::MAX);
                        arena.spec.store_atomic::<$type>(data_addr, 0, Ordering::SeqCst);
                        assert_eq!(arena.load::<$type>(data_addr), 0);
                        // Only the affected bits are set to 0
                        assert_eq!(arena.load_raw::<$type>(), <$type>::MAX & (!max_value));
                    });
                }

                #[test]
                fn [<$tname _compare_exchange_success>]() {
                    test_side_metadata($log_bits, |arena| {
                        let data_addr = arena.data_addr;
                        let max_value: $type = max_value($log_bits) as _;
                        arena.store_raw::<$type>(<$type>::MAX);
                        arena.spec.store_atomic::<$type>(data_addr, 1, Ordering::SeqCst);

                        let old_val = arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        assert_eq!(old_val, 1);

                        let new_val = 0;
                        let res = arena.spec.compare_exchange_atomic::<$type>(data_addr, old_val, new_val, Ordering::SeqCst, Ordering::SeqCst);
                        assert!(res.is_ok());
                        assert_eq!(res.unwrap(), old_val, "old vals do not match");

                        let after_update = arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        assert_eq!(after_update, new_val);
                        assert_eq!(arena.load_raw::<$type>(), <$type>::MAX & (!max_value));
                    });
                }

                #[test]
                fn [<$tname _compare_exchange_fail>]() {
                    test_side_metadata($log_bits, |arena| {
                        let data_addr = arena.data_addr;
                        arena.store_raw::<$type>(<$type>::MAX);
                        arena.spec.store_atomic::<$type>(data_addr, 1, Ordering::SeqCst);

                        let old_val = arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        assert_eq!(old_val, 1);

                        // make old_val outdated
                        arena.spec.store_atomic::<$type>(data_addr, 0, Ordering::SeqCst);
                        let bits_before_cas = arena.load_raw::<$type>();

                        let new_val = 0;
                        let res = arena.spec.compare_exchange_atomic::<$type>(data_addr, old_val, new_val, Ordering::SeqCst, Ordering::SeqCst);
                        assert!(res.is_err());
                        assert_eq!(res.err().unwrap(), 0);
                        let bits_after_cas = arena.load_raw::<$type>();
                        assert_eq!(bits_before_cas, bits_after_cas);
                    });
                }

                #[test]
                fn [<$tname _fetch_add_1>]() {
                    test_side_metadata($log_bits, |arena| {
                        let data_addr = arena.data_addr;
                        arena.store_raw::<$type>(<$type>::MAX);
                        arena.spec.store_atomic::<$type>(data_addr, 0, Ordering::SeqCst);

                        let old_val = arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        let old_val_from_fetch = arena.spec.fetch_add_atomic::<$type>(data_addr, 1, Ordering::SeqCst);
                        assert_eq!(old_val_from_fetch, old_val);

                        let new_val = arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        assert_eq!(new_val, 1);
                    });
                }

                #[test]
                fn [<$tname _fetch_add_max>]() {
                    test_side_metadata($log_bits, |arena| {
                        let data_addr = arena.data_addr;
                        let max_value: $type = max_value($log_bits) as _;
                        arena.store_raw::<$type>(<$type>::MAX);
                        arena.spec.store_atomic::<$type>(data_addr, 0, Ordering::SeqCst);

                        let old_val = arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        let old_val_from_fetch = arena.spec.fetch_add_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);
                        assert_eq!(old_val_from_fetch, old_val);

                        let new_val = arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        assert_eq!(new_val, max_value);
                    });
                }

                #[test]
                fn [<$tname _fetch_add_overflow>]() {
                    test_side_metadata($log_bits, |arena| {
                        let data_addr = arena.data_addr;
                        let max_value: $type = max_value($log_bits) as _;
                        arena.store_raw::<$type>(<$type>::MAX);
                        arena.spec.store_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);

                        let old_val = arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        let old_val_from_fetch = arena.spec.fetch_add_atomic::<$type>(data_addr, 1, Ordering::SeqCst);
                        assert_eq!(old_val_from_fetch, old_val);

                        let new_val = arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        assert_eq!(new_val, 0);
                    });
                }

                #[test]
                fn [<$tname _fetch_sub_1>]() {
                    test_side_metadata($log_bits, |arena| {
                        let data_addr = arena.data_addr;
                        arena.store_raw::<$type>(<$type>::MAX);
                        arena.spec.store_atomic::<$type>(data_addr, 1, Ordering::SeqCst);

                        let old_val = arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        let old_val_from_fetch = arena.spec.fetch_sub_atomic::<$type>(data_addr, 1, Ordering::SeqCst);
                        assert_eq!(old_val_from_fetch, old_val);

                        let new_val = arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        assert_eq!(new_val, 0);
                    });
                }

                #[test]
                fn [<$tname _fetch_sub_max>]() {
                    test_side_metadata($log_bits, |arena| {
                        let data_addr = arena.data_addr;
                        let max_value: $type = max_value($log_bits) as _;
                        arena.store_raw::<$type>(<$type>::MAX);
                        arena.spec.store_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);

                        let old_val = arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        let old_val_from_fetch = arena.spec.fetch_sub_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);
                        assert_eq!(old_val_from_fetch, old_val);

                        let new_val = arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        assert_eq!(new_val, 0);
                    });
                }

                #[test]
                fn [<$tname _fetch_sub_overflow>]() {
                    test_side_metadata($log_bits, |arena| {
                        let data_addr = arena.data_addr;
                        let max_value: $type = max_value($log_bits) as _;
                        arena.store_raw::<$type>(<$type>::MAX);
                        arena.spec.store_atomic::<$type>(data_addr, 0, Ordering::SeqCst);

                        let old_val = arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        let old_val_from_fetch = arena.spec.fetch_sub_atomic::<$type>(data_addr, 1, Ordering::SeqCst);
                        assert_eq!(old_val_from_fetch, old_val);

                        let new_val = arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        assert_eq!(new_val, max_value);
                    });
                }

                #[test]
                fn [<$tname _fetch_and>]() {
                    test_side_metadata($log_bits, |arena| {
                        let data_addr = arena.data_addr;
                        let max_value: $type = max_value($log_bits) as _;
                        arena.store_raw::<$type>(<$type>::MAX);
                        arena.spec.store_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);

                        let old_val = arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        let old_val_from_fetch = arena.spec.fetch_and_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);
                        assert_eq!(old_val_from_fetch, old_val, "old values do not match");
                        assert_eq!(arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst), max_value, "load values do not match");
                        assert_eq!(arena.load_raw::<$type>(), <$type>::MAX, "raw values do not match");

                        let last_bit_zero = max_value - 1;
                        let old_val = arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        let old_val_from_fetch = arena.spec.fetch_and_atomic::<$type>(data_addr, last_bit_zero, Ordering::SeqCst);
                        assert_eq!(old_val_from_fetch, old_val);
                        assert_eq!(arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst), last_bit_zero);
                        assert_eq!(arena.load_raw::<$type>(), <$type>::MAX - 1);
                    });
                }

                #[test]
                fn [<$tname _fetch_or>]() {
                    test_side_metadata($log_bits, |arena| {
                        let data_addr = arena.data_addr;
                        let max_value: $type = max_value($log_bits) as _;
                        arena.store_raw::<$type>(0);
                        arena.spec.store_atomic::<$type>(data_addr, 0, Ordering::SeqCst);

                        let old_val = arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        let old_val_from_fetch = arena.spec.fetch_or_atomic::<$type>(data_addr, 0, Ordering::SeqCst);
                        assert_eq!(old_val_from_fetch, old_val);
                        assert_eq!(arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst), 0);
                        assert_eq!(arena.load_raw::<$type>(), 0);

                        let old_val = arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        let old_val_from_fetch = arena.spec.fetch_or_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);
                        assert_eq!(old_val_from_fetch, old_val);
                        assert_eq!(arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst), max_value);
                        assert_eq!(arena.load_raw::<$type>(), max_value);
                    });
                }

                #[test]
                fn [<$tname _fetch_update_success>]() {
                    test_side_metadata($log_bits, |arena| {
                        let data_addr = arena.data_addr;
                        let max_value: $type = max_value($log_bits) as _;
                        arena.store_raw::<$type>(<$type>::MAX);
                        arena.spec.store_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);

                        let old_val = arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        let fetch_res = arena.spec.fetch_update_atomic::<$type, _>(data_addr, Ordering::SeqCst, Ordering::SeqCst, |_x: $type| Some(0));
                        assert!(fetch_res.is_ok());
                        assert_eq!(fetch_res.unwrap(), old_val);
                        assert_eq!(arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst), 0);
                        assert_eq!(arena.load_raw::<$type>(), <$type>::MAX & (!max_value));
                    });
                }

                #[test]
                fn [<$tname _fetch_update_fail>]() {
                    test_side_metadata($log_bits, |arena| {
                        let data_addr = arena.data_addr;
                        let max_value: $type = max_value($log_bits) as _;
                        arena.store_raw::<$type>(<$type>::MAX);
                        arena.spec.store_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);

                        let old_val = arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst);
                        let fetch_res = arena.spec.fetch_update_atomic::<$type, _>(data_addr, Ordering::SeqCst, Ordering::SeqCst, |_x: $type| None);
                        assert!(fetch_res.is_err());
                        assert_eq!(fetch_res.err().unwrap(), old_val);
                        assert_eq!(arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst), max_value);
                        assert_eq!(arena.load_raw::<$type>(), <$type>::MAX);
                    });
                }

                #[test]
                fn [<$tname _find_prev_non_zero_value_easy>]() {
                    test_side_metadata($log_bits, |arena| {
                        let data_addr = arena.data_addr;
                        let max_value: $type = max_value($log_bits) as _;
                        arena.spec.store_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);

                        let res_addr = arena.find_prev::<$type>(data_addr, 8);
                        assert_eq!(res_addr, Some(data_addr));
                    });
                }

                #[test]
                fn [<$tname _find_prev_non_zero_value_arbitrary_bytes>]() {
                    test_side_metadata($log_bits, |arena| {
                        let data_addr = arena.data_addr;
                        let max_value: $type = max_value($log_bits) as _;
                        arena.spec.store_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);

                        let test_region = 1 << TEST_LOG_BYTES_IN_REGION;
                        for len in 1..(test_region * 4) {
                            let start_addr = data_addr + len;
                            let res_addr = arena.find_prev::<$type>(start_addr, len + 1);
                            assert_eq!(res_addr, Some(data_addr));
                        }
                    });
                }

                #[test]
                fn [<$tname _find_prev_non_zero_value_arbitrary_start>]() {
                    test_side_metadata($log_bits, |arena| {
                        let data_addr = arena.data_addr;
                        let max_value: $type = max_value($log_bits) as _;

                        for offset in 0..7usize {
                            let test_data_addr = data_addr + offset;
                            arena.spec.store_atomic::<$type>(test_data_addr, max_value, Ordering::SeqCst);
                            assert_eq!(arena.find_prev::<$type>(test_data_addr, 4096), Some(data_addr));
                            arena.spec.store_atomic::<$type>(test_data_addr, 0, Ordering::SeqCst);
                        }
                    });
                }

                #[test]
                fn [<$tname _find_prev_non_zero_value_no_find>]() {
                    test_side_metadata($log_bits, |arena| {
                        let data_addr = arena.data_addr;
                        arena.spec.store_atomic::<$type>(data_addr, 0, Ordering::SeqCst);

                        let test_region = 1 << TEST_LOG_BYTES_IN_REGION;
                        for len in 1..(test_region * 4) {
                            let start_addr = data_addr + len;
                            assert_eq!(arena.find_prev::<$type>(start_addr, len + 1), None);
                        }
                    });
                }

                #[test]
                fn [<$tname _find_next_non_zero_value_easy>]() {
                    test_side_metadata($log_bits, |arena| {
                        let data_addr = arena.data_addr;
                        let max_value: $type = max_value($log_bits) as _;
                        arena.spec.store_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);

                        let res_addr = arena.find_next::<$type>(data_addr, 8);
                        assert_eq!(res_addr, Some(data_addr));
                    });
                }

                #[test]
                fn [<$tname _find_next_non_zero_value_arbitrary_bytes>]() {
                    test_side_metadata($log_bits, |arena| {
                        let max_value: $type = max_value($log_bits) as _;
                        let test_region = 1 << TEST_LOG_BYTES_IN_REGION;
                        let data_addr = arena.data_addr + test_region * 4;

                        arena.spec.store_atomic::<$type>(data_addr, max_value, Ordering::SeqCst);
                        assert_eq!(arena.spec.load_atomic::<$type>(data_addr, Ordering::SeqCst), max_value);

                        for len in 1..(test_region * 4) {
                            let start_addr = data_addr - len;
                            assert_eq!(arena.find_next::<$type>(start_addr, len + 1), Some(data_addr));
                        }
                    });
                }

                #[test]
                fn [<$tname _find_next_non_zero_value_arbitrary_start>]() {
                    test_side_metadata($log_bits, |arena| {
                        let data_addr = arena.data_addr;
                        let max_value: $type = max_value($log_bits) as _;

                        for offset in 0..7usize {
                            let test_data_addr = data_addr + offset;
                            arena.spec.store_atomic::<$type>(test_data_addr, max_value, Ordering::SeqCst);
                            assert_eq!(arena.find_next::<$type>(test_data_addr, 4096), Some(data_addr));
                            arena.spec.store_atomic::<$type>(test_data_addr, 0, Ordering::SeqCst);
                        }
                    });
                }

                #[test]
                fn [<$tname _find_next_non_zero_value_no_find>]() {
                    test_side_metadata($log_bits, |arena| {
                        let data_addr = arena.data_addr;
                        arena.spec.store_atomic::<$type>(data_addr, 0, Ordering::SeqCst);

                        let test_region = 1 << TEST_LOG_BYTES_IN_REGION;
                        for len in 1..(test_region * 4) {
                            let start_addr = data_addr - len;
                            assert_eq!(arena.find_next::<$type>(start_addr, len + 1), None);
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
        test_side_metadata(1, |arena| {
            let spec = &arena.spec;
            let data_addr = arena.data_addr;
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
        test_side_metadata(3, |arena| {
            let spec = &arena.spec;
            let data_addr = arena.data_addr;
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
                // SAFETY: `spec` is mapped for `[data_addr, data_addr + four_pages)`, and only
                // `AtomicU8` references touch the 4 mapped, initialized metadata bytes here.
                let atomics = unsafe { spec.mapped_atomic_bytes_unchecked::<4>(data_addr) };
                for (slot, val) in atomics.iter().zip([0x11, 0x22, 0x33, 0x44]) {
                    slot.store(val, Ordering::Relaxed);
                }
                assert_eq!(atomics[0].load(Ordering::Relaxed), 0x11);
                assert_eq!(atomics[1].load(Ordering::Relaxed), 0x22);
                atomics[2].store(0x99, Ordering::Relaxed);
                assert_eq!(atomics[2].load(Ordering::Relaxed), 0x99);
                assert_eq!(atomics[3].load(Ordering::Relaxed), 0x44);
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
            let _ = spec.validate_mapped_atomic_bytes(unaligned_data, 1);
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
        let _ = spec.validate_mapped_atomic_bytes(Address::ZERO + 4096usize, 1);
    }

    #[test]
    fn test_zero_size_and_unbounded_boundary_regressions() {
        test_side_metadata(0, |arena| {
            let spec = &arena.spec;
            let data_addr = arena.data_addr;
            let region = 1usize << TEST_LOG_BYTES_IN_REGION;

            // Zero-size bulk updates / copies must be no-ops without touching neighboring bits.
            spec.store_atomic::<u8>(data_addr, 1, Ordering::SeqCst);
            spec.bzero_metadata(data_addr, 0);
            assert_eq!(spec.load_atomic::<u8>(data_addr, Ordering::SeqCst), 1);
            spec.bcopy_metadata_contiguous(data_addr, 0, spec);
            assert_eq!(spec.load_atomic::<u8>(data_addr, Ordering::SeqCst), 1);

            // Zero-limit and `usize::MAX`-limit searches must not underflow/overflow `Address`.
            assert_eq!(arena.find_prev::<u8>(data_addr, 0), None);
            assert_eq!(arena.find_next::<u8>(data_addr, 0), None);
            assert_eq!(
                arena.find_prev::<u8>(data_addr, usize::MAX),
                Some(data_addr)
            );
            assert_eq!(
                arena.find_next::<u8>(data_addr, usize::MAX),
                Some(data_addr)
            );

            spec.store_atomic::<u8>(data_addr, 0, Ordering::SeqCst);
            spec.store_atomic::<u8>(data_addr + region, 1, Ordering::SeqCst);
            assert_eq!(
                arena.find_next::<u8>(data_addr, usize::MAX),
                Some(data_addr + region)
            );
            spec.store_atomic::<u8>(data_addr + region, 0, Ordering::SeqCst);
            spec.store_atomic::<u8>(data_addr, 1, Ordering::SeqCst);
            assert_eq!(
                arena.find_prev::<u8>(data_addr + region, usize::MAX),
                Some(data_addr)
            );
            spec.store_atomic::<u8>(data_addr, 0, Ordering::SeqCst);

            let grain_end_meta =
                arena.meta_addr.align_down(MMAPPER.granularity()) + MMAPPER.granularity();
            let last_in_grain =
                contiguous_meta_address_to_address(spec, grain_end_meta - 1usize, 7);
            assert_eq!(arena.find_next::<u8>(last_in_grain, usize::MAX), None);
            assert_eq!(
                arena.find_next::<u8>(Address::MAX - (region - 1), usize::MAX),
                None
            );

            // Top-of-address-space `[CONTIGUOUS_DATA_MAX - 63, CONTIGUOUS_DATA_MAX + 1)` range
            // translation and sanity verification must not overflow.
            let word_spec = SideMetadataSpec {
                name: "top_of_memory_bit_spec",
                is_global: true,
                offset: 0,
                log_num_of_bits: 0,
                log_bytes_in_region: 3,
            };
            let top_64 = (Address::ZERO + helpers::CONTIGUOUS_DATA_MAX) - 63usize;
            let (ms, msb, me, meb) =
                helpers::half_open_contiguous_meta_bit_range(&word_spec, top_64, 64).unwrap();
            assert_eq!((msb, meb), (0, 0));
            assert_eq!(me - ms, 1);
            #[cfg(feature = "extreme_assertions")]
            {
                let max_top = Address::MAX - (region - 1);
                sanity::verify_bset(spec, max_top, region);
                sanity::verify_bcopy(spec, max_top, region, spec);
                sanity::verify_bzero(spec, max_top, region);
            }
        });
    }
}
