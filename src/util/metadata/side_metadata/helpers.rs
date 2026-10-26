use super::ranges::BitOffset;
use super::SideMetadataSpec;
use crate::util::constants::LOG_BYTES_IN_PAGE;
use crate::util::constants::{BITS_IN_WORD, BYTES_IN_PAGE, LOG_BITS_IN_BYTE};
use crate::util::conversions::rshift_align_up;
use crate::util::heap::layout::vm_layout::VMLayout;
#[cfg(target_pointer_width = "32")]
use crate::util::metadata::side_metadata::address_to_chunked_meta_address;
use crate::util::os::*;
use crate::util::Address;
use crate::MMAPPER;

/// Performs address translation in contiguous metadata spaces (e.g. global and policy-specific in 64-bits, and global in 32-bits)
#[inline(always)]
pub(crate) fn address_to_contiguous_meta_address(
    metadata_spec: &SideMetadataSpec,
    data_addr: Address,
) -> Address {
    let log_bits_num = metadata_spec.log_num_of_bits as i32;
    let log_bytes_in_region = metadata_spec.log_bytes_in_region;

    let shift = (LOG_BITS_IN_BYTE as i32) - log_bits_num;

    if shift >= 0 {
        metadata_spec.get_starting_address() + ((data_addr >> log_bytes_in_region) >> shift)
    } else {
        metadata_spec.get_starting_address() + ((data_addr >> log_bytes_in_region) << (-shift))
    }
}

/// Performs reverse address translation from contiguous metadata bits to data addresses.
/// The input address and bit shift should be aligned.
///
/// Arguments:
/// * `metadata_spec`: The side metadata spec. It should be contiguous side metadata.
/// * `metadata_addr`; The metadata address. Returned by [`address_to_contiguous_meta_address`].
/// * `bit`: The bit shift for the metadata. Returned by [`meta_byte_lshift`].
pub(crate) fn contiguous_meta_address_to_address(
    metadata_spec: &SideMetadataSpec,
    metadata_addr: Address,
    bit: u8,
) -> Address {
    debug_assert_eq!(
        align_metadata_address(metadata_spec, metadata_addr, bit),
        (metadata_addr, bit)
    );
    let shift = (LOG_BITS_IN_BYTE as i32) - metadata_spec.log_num_of_bits as i32;
    let relative_meta_addr = metadata_addr - metadata_spec.get_starting_address();

    let data_addr_intermediate = if shift >= 0 {
        relative_meta_addr << shift
    } else {
        relative_meta_addr >> (-shift)
    };
    let data_addr_bit_shift = if shift >= 0 {
        metadata_spec.log_bytes_in_region - metadata_spec.log_num_of_bits
    } else {
        metadata_spec.log_bytes_in_region
    };

    let data_addr = (data_addr_intermediate << metadata_spec.log_bytes_in_region)
        + ((bit as usize) << data_addr_bit_shift);

    // SAFETY: `data_addr` is reconstructed inversely from a contiguous side-metadata offset and bit
    // index; `Address::from_usize` marks raw integer-to-address conversion without dereferencing it.
    unsafe { Address::from_usize(data_addr) }
}

/// Align an pair of a metadata address and a metadata bit offset to the start of this metadata value.
/// For example, when the metadata is 4 bits, it should only start at bit 0 or bit 4.
/// When the metadata is 16 bits, it should only start at bit 0, and its metadata address should be aligned to 2 bytes.
/// This is important, as [`contiguous_meta_address_to_address`] can only convert the start address of metadata to
/// the data address.
pub(super) fn align_metadata_address(
    spec: &SideMetadataSpec,
    metadata_addr: Address,
    bit: u8,
) -> (Address, u8) {
    if spec.log_num_of_bits >= LOG_BITS_IN_BYTE as usize {
        (
            metadata_addr.align_down(1 << (spec.log_num_of_bits - LOG_BITS_IN_BYTE as usize)),
            0,
        )
    } else {
        (
            metadata_addr,
            crate::util::conversions::raw_align_down(
                bit as usize,
                (1 << spec.log_num_of_bits) as usize,
            ) as u8,
        )
    }
}

/// Unmaps the specified metadata range, or panics.
#[cfg(test)]
pub(crate) fn ensure_munmap_metadata(start: Address, size: usize) {
    trace!("ensure_munmap_metadata({}, 0x{:x})", start, size);

    assert!(OS::munmap(start, size).is_ok())
}

/// Unmaps a metadata space (`spec`) for the specified data address range (`start` and `size`)
/// Returns the size in bytes that get munmapped.
#[cfg(test)]
pub(crate) fn ensure_munmap_contiguous_metadata_space(
    start: Address,
    size: usize,
    spec: &SideMetadataSpec,
) -> usize {
    // nearest page-aligned starting address
    let metadata_start = address_to_meta_address(spec, start);
    let mmap_start = metadata_start.align_down(BYTES_IN_PAGE);
    // nearest page-aligned ending address
    let metadata_size = data_to_meta_size_round_up(spec, size);
    let mmap_size = (metadata_start + metadata_size).align_up(BYTES_IN_PAGE) - mmap_start;
    if mmap_size > 0 {
        ensure_munmap_metadata(mmap_start, mmap_size);
    }
    mmap_size
}

/// Tries to mmap the metadata space (`spec`) for the specified data address range (`start` and `size`).
/// Setting `no_reserve` to true means the function will only map address range, without reserving swap-space/physical memory.
/// Returns the size in bytes that gets mmapped in the function if success.
pub(super) fn try_mmap_contiguous_metadata_space(
    start: Address,
    size: usize,
    spec: &SideMetadataSpec,
    no_reserve: bool,
    anno: &MmapAnnotation,
) -> MmapResult<usize> {
    debug_assert!(start.is_aligned_to(BYTES_IN_PAGE));
    debug_assert!(size % BYTES_IN_PAGE == 0);

    // nearest page-aligned starting address
    let metadata_start = address_to_meta_address(spec, start);
    let mmap_start = metadata_start.align_down(BYTES_IN_PAGE);
    // nearest page-aligned ending address
    let metadata_size = data_to_meta_size_round_up(spec, size);
    let mmap_size = (metadata_start + metadata_size).align_up(BYTES_IN_PAGE) - mmap_start;
    if mmap_size > 0 {
        if !no_reserve {
            MMAPPER.ensure_mapped(
                mmap_start,
                mmap_size >> LOG_BYTES_IN_PAGE,
                HugePageSupport::No,
                MmapProtection::ReadWrite,
                anno,
            )
        } else {
            MMAPPER.quarantine_address_range(
                mmap_start,
                mmap_size >> LOG_BYTES_IN_PAGE,
                HugePageSupport::No,
                anno,
            )
        }
        .map(|_| mmap_size)
        .inspect_err(|_| {
            warn!(
                "Failed to mmap metadata space: {} - {}",
                mmap_start,
                mmap_start + mmap_size
            );
        })
    } else {
        Ok(0)
    }
}

/// Performs the translation of data address (`data_addr`) to metadata address for the specified metadata (`metadata_spec`).
#[inline(always)]
pub(crate) fn address_to_meta_address(
    metadata_spec: &SideMetadataSpec,
    data_addr: Address,
) -> Address {
    #[cfg(target_pointer_width = "32")]
    let res = {
        if metadata_spec.is_global {
            address_to_contiguous_meta_address(metadata_spec, data_addr)
        } else {
            address_to_chunked_meta_address(metadata_spec, data_addr)
        }
    };
    #[cfg(target_pointer_width = "64")]
    let res = { address_to_contiguous_meta_address(metadata_spec, data_addr) };

    trace!(
        "address_to_meta_address({:?}, addr: {}) -> 0x{:x}",
        metadata_spec,
        data_addr,
        res
    );

    res
}

/// Return the base-2 logarithm of the ratio of data bits and metadata bits per region.
///
/// Suppose a memory region has `data_bits` bits of data, and `meta_bits` bits of metadata for
/// `metadata_spec`, and the result of `log_data_meta_ratio(metadata_spec)` is `shift`, then
///
/// -   `data_bits >> shift == meta_bits`
/// -   `meta_bits << shift == data_bits`
pub(super) const fn log_data_meta_ratio(metadata_spec: &SideMetadataSpec) -> usize {
    let log_data_bits_in_region = (LOG_BITS_IN_BYTE as usize) + metadata_spec.log_bytes_in_region;
    let log_meta_bits_in_region = metadata_spec.log_num_of_bits;

    // TODO: In theory, it is possible to construct a side metadata that has more metadata bits than
    // data bits per region.  But such pathological side metadata consumes way too much memory, and
    // should never be used in any useful applications.  It should be forbidden.
    log_data_bits_in_region - log_meta_bits_in_region
}

/// Calculate the amount of metadata needed for the give amount of data memory, round up to nearest
/// integer. `data_size` can be in any unit, e.g. bits, bytes, pages, blocks, chunks, etc., and the
/// result has the same unit.
pub(super) const fn data_to_meta_size_round_up(
    metadata_spec: &SideMetadataSpec,
    data_size: usize,
) -> usize {
    rshift_align_up(data_size, log_data_meta_ratio(metadata_spec))
}

/// Calculate the amount of data governed by the give amount of metadata.  `meta_size` can be in any
/// unit, e.g. bits, bytes, pages, blocks, chunks, etc., and the result has the same unit.
pub(super) const fn meta_to_data_size(metadata_spec: &SideMetadataSpec, meta_size: usize) -> usize {
    meta_size << log_data_meta_ratio(metadata_spec)
}

pub(super) const fn metadata_address_range_size(metadata_spec: &SideMetadataSpec) -> usize {
    1usize << (VMLayout::LOG_ARCH_ADDRESS_SPACE - log_data_meta_ratio(metadata_spec))
}

/// Highest inclusive data address covered by contiguous side-metadata spaces (`(1 << 48) - 1` on
/// 64-bit architectures; `usize::MAX` on 32-bit architectures).
#[cfg(target_pointer_width = "64")]
pub(crate) const CONTIGUOUS_DATA_MAX: usize = (1usize << VMLayout::LOG_ARCH_ADDRESS_SPACE) - 1;
#[cfg(target_pointer_width = "32")]
pub(crate) const CONTIGUOUS_DATA_MAX: usize = usize::MAX;

#[inline(always)]
pub(crate) fn meta_byte_lshift(metadata_spec: &SideMetadataSpec, data_addr: Address) -> u8 {
    let bits_num_log = metadata_spec.log_num_of_bits as i32;
    if bits_num_log >= 3 {
        return 0;
    }
    let rem_shift = BITS_IN_WORD as i32 - ((LOG_BITS_IN_BYTE as i32) - bits_num_log);
    ((((data_addr >> metadata_spec.log_bytes_in_region) << rem_shift) >> rem_shift) << bits_num_log)
        as u8
}

#[inline(always)]
pub(super) fn meta_byte_mask(metadata_spec: &SideMetadataSpec) -> u8 {
    let bits_num_log = metadata_spec.log_num_of_bits;
    ((1usize << (1usize << bits_num_log)) - 1) as u8
}

/// Convert a half-open metadata bit range `[start_addr:start_bit, end_addr:end_bit)` (`start_bit < 8`,
/// `end_bit < 8`) into the non-empty byte span `[byte_start, byte_end)` and boundary byte masks
/// `(first_mask, last_mask)`. Returns `None` if the bit range is empty.
#[inline]
pub(crate) fn active_bit_range_bytes(
    start_addr: Address,
    start_bit: u8,
    end_addr: Address,
    end_bit: u8,
) -> Option<(Address, Address, u8, u8)> {
    debug_assert!(start_bit < 8 && end_bit < 8);
    if start_addr > end_addr || (start_addr == end_addr && start_bit >= end_bit) {
        return None;
    }
    let byte_start = start_addr;
    let byte_end = if end_bit == 0 {
        end_addr
    } else {
        end_addr + 1usize
    };
    let first_mask = 0xffu8 << start_bit;
    let last_mask = if end_bit == 0 {
        0xffu8
    } else {
        (1u8 << end_bit) - 1
    };
    Some((byte_start, byte_end, first_mask, last_mask))
}

#[inline]
fn contiguous_end_after_region(spec: &SideMetadataSpec, last_region: Address) -> (Address, u8) {
    let last_meta = address_to_contiguous_meta_address(spec, last_region);
    if spec.log_num_of_bits < LOG_BITS_IN_BYTE as usize {
        let next_bit = meta_byte_lshift(spec, last_region) + (1u8 << spec.log_num_of_bits);
        if next_bit == 8 {
            (last_meta + 1usize, 0)
        } else {
            (last_meta, next_bit)
        }
    } else {
        let entry_bytes = 1usize << (spec.log_num_of_bits - LOG_BITS_IN_BYTE as usize);
        (last_meta + entry_bytes, 0)
    }
}

/// Convert a half-open data range `[start, start + size)` into a contiguous side-metadata bit range
/// `(meta_start, meta_start_bit, meta_end, meta_end_bit)` without overflowing when `start + size`
/// reaches `CONTIGUOUS_DATA_MAX + 1` (`2^32` on 32-bit, `1 << 48` on 64-bit). Returns `None` if
/// `size == 0`.
#[inline]
pub(crate) fn half_open_contiguous_meta_bit_range(
    spec: &SideMetadataSpec,
    start: Address,
    size: usize,
) -> Option<(Address, u8, Address, u8)> {
    if size == 0 {
        return None;
    }
    let last_raw = start
        .as_usize()
        .checked_add(size - 1)
        .expect("Side-metadata range overflowed Address");
    assert!(
        last_raw <= CONTIGUOUS_DATA_MAX,
        "Contiguous side-metadata range overflowed architectural address space: start={start}, size={size:#x}, spec={spec:?}"
    );
    let meta_start = address_to_contiguous_meta_address(spec, start);
    let meta_start_bit = meta_byte_lshift(spec, start);
    let (meta_end, meta_end_bit) = match start.as_usize().checked_add(size) {
        Some(end_raw) if end_raw <= CONTIGUOUS_DATA_MAX => {
            let end_addr = start + size;
            (
                address_to_contiguous_meta_address(spec, end_addr),
                meta_byte_lshift(spec, end_addr),
            )
        }
        _ => {
            let last_region = (start + (CONTIGUOUS_DATA_MAX - start.as_usize()))
                .align_down(1usize << spec.log_bytes_in_region);
            contiguous_end_after_region(spec, last_region)
        }
    };
    Some((meta_start, meta_start_bit, meta_end, meta_end_bit))
}

/// Convert a half-open data range `[start, start + size)` into the contiguous side-metadata byte
/// span `(byte_start, byte_end, first_mask, last_mask)`.
#[inline]
pub(crate) fn half_open_contiguous_meta_range(
    spec: &SideMetadataSpec,
    start: Address,
    size: usize,
) -> Option<(Address, Address, u8, u8)> {
    let (ms, msb, me, meb) = half_open_contiguous_meta_bit_range(spec, start, size)?;
    active_bit_range_bytes(ms, msb, me, meb)
}

/// Convert an inclusive region-aligned data range `first_region..=last_region` (`first_region <=
/// last_region`) into the contiguous side-metadata byte span `(byte_start, byte_end, first_mask,
/// last_mask)` without adding `region_bytes` to `last_region`.
#[inline]
pub(crate) fn inclusive_contiguous_meta_range(
    spec: &SideMetadataSpec,
    first_region: Address,
    last_region: Address,
) -> Option<(Address, Address, u8, u8)> {
    if first_region > last_region {
        return None;
    }
    let start_meta = address_to_contiguous_meta_address(spec, first_region);
    let start_bit = meta_byte_lshift(spec, first_region);
    let (end_meta, end_bit) = contiguous_end_after_region(spec, last_region);
    active_bit_range_bytes(start_meta, start_bit, end_meta, end_bit)
}

/// Invoke `f(sub_start, sub_size)` once for contiguous side-metadata specs, or split `[start,
/// start + size)` across `BYTES_IN_CHUNK` boundaries on 32-bit chunked side-metadata specs without
/// overflowing at the top chunk `[0xffc0_0000, 2^32)`.
#[inline]
pub(crate) fn for_each_spec_data_span(
    spec: &SideMetadataSpec,
    start: Address,
    size: usize,
    mut f: impl FnMut(Address, usize),
) {
    if size == 0 {
        return;
    }
    if spec.uses_contiguous_side_metadata() {
        f(start, size);
    } else {
        #[cfg(target_pointer_width = "32")]
        {
            use crate::util::heap::layout::vm_layout::BYTES_IN_CHUNK;
            let last_raw = start
                .as_usize()
                .checked_add(size - 1)
                .expect("Chunked side-metadata range overflowed Address");
            let last_byte = start + (last_raw - start.as_usize());
            let first_chunk = start.align_down(BYTES_IN_CHUNK);
            let last_chunk = last_byte.align_down(BYTES_IN_CHUNK);
            if first_chunk == last_chunk {
                f(start, size);
            } else {
                let second_chunk = first_chunk + BYTES_IN_CHUNK;
                f(start, second_chunk - start);
                let mut chunk = second_chunk;
                while chunk < last_chunk {
                    f(chunk, BYTES_IN_CHUNK);
                    chunk += BYTES_IN_CHUNK;
                }
                f(last_chunk, (last_byte - last_chunk) + 1);
            }
        }
    }
}

/// Verify that `[meta_start, meta_end)` lies strictly within both `[reserved_start, reserved_end)`
/// and `spec`'s window.
#[inline(always)]
pub(crate) fn assert_meta_range_within_reserved(
    spec: &SideMetadataSpec,
    chunk_anchor: Address,
    reserved_start: Address,
    reserved_end: Address,
    meta_start: Address,
    meta_end: Address,
) {
    assert!(
        !meta_start.is_zero()
            && meta_start <= meta_end
            && meta_start >= reserved_start
            && meta_end <= reserved_end,
        "Side-metadata range [{meta_start}, {meta_end}) outside reserved window [{reserved_start}, {reserved_end}) for {spec:?}"
    );
    if spec.uses_contiguous_side_metadata() {
        let range_bytes = metadata_address_range_size(spec);
        let spec_start_raw = reserved_start
            .as_usize()
            .checked_add(spec.offset)
            .expect("Contiguous side-metadata spec offset overflow");
        let spec_end_raw = spec_start_raw
            .checked_add(range_bytes)
            .expect("Contiguous side-metadata spec upper bound overflow");
        assert!(
            meta_start.as_usize() >= spec_start_raw && meta_end.as_usize() <= spec_end_raw,
            "Contiguous side-metadata range [{meta_start}, {meta_end}) out of bounds [{spec_start_raw:#x}, {spec_end_raw:#x}) for {spec:?}"
        );
    }
    #[cfg(target_pointer_width = "32")]
    if spec.uses_chunked_side_metadata() {
        let chunk_meta_base = super::address_to_meta_chunk_addr(chunk_anchor).as_usize();
        let spec_start_raw = chunk_meta_base
            .checked_add(spec.get_offset_for_chunked())
            .expect("Chunked side-metadata spec offset overflow");
        let spec_end_raw = chunk_meta_base
            .checked_add(spec.upper_bound_offset())
            .expect("Chunked side-metadata spec upper bound overflow");
        assert!(
            meta_start.as_usize() >= spec_start_raw && meta_end.as_usize() <= spec_end_raw,
            "Chunked side-metadata range [{meta_start}, {meta_end}) out of bounds [{spec_start_raw:#x}, {spec_end_raw:#x}) for {spec:?}"
        );
    }
    #[cfg(target_pointer_width = "64")]
    let _ = chunk_anchor;
}

/// Assert that side metadata is initialized and `[meta_start, meta_end)` lies strictly within both
/// MMTk's startup-quarantined side-metadata reservation and `spec`'s window.
#[inline(always)]
pub(crate) fn assert_meta_range_in_spec_bounds(
    spec: &SideMetadataSpec,
    chunk_anchor: Address,
    meta_start: Address,
    meta_end: Address,
) {
    let (reserved_start, reserved_end) = super::layout::try_side_metadata_reserved_range()
        .unwrap_or_else(|| panic!("Side metadata is not initialized for {:?}", spec));
    assert_meta_range_within_reserved(
        spec,
        chunk_anchor,
        reserved_start,
        reserved_end,
        meta_start,
        meta_end,
    );
}

#[inline]
pub(crate) fn grain_rem(addr: Address, grain: usize) -> usize {
    addr.align_down(grain).as_usize().saturating_add(grain) - addr.as_usize()
}

/// Split `[byte_start, byte_end)` at `MMAPPER.granularity()` boundaries in ascending order.
#[inline]
pub(crate) fn try_for_each_meta_grain_forward<B>(
    byte_start: Address,
    byte_end: Address,
    first_mask: u8,
    last_mask: u8,
    mut f: impl FnMut(Address, Address, u8, u8) -> core::ops::ControlFlow<B>,
) -> Option<B> {
    let grain = MMAPPER.granularity();
    let mut g_start = byte_start;
    while g_start < byte_end {
        let g_end = g_start + (byte_end - g_start).min(grain_rem(g_start, grain));
        let g_first = if g_start == byte_start {
            first_mask
        } else {
            0xff
        };
        let g_last = if g_end == byte_end { last_mask } else { 0xff };
        if let core::ops::ControlFlow::Break(val) = f(g_start, g_end, g_first, g_last) {
            return Some(val);
        }
        g_start = g_end;
    }
    None
}

/// Split `[byte_start, byte_end)` at `MMAPPER.granularity()` boundaries in descending order.
#[inline]
pub(crate) fn try_for_each_meta_grain_backward<B>(
    byte_start: Address,
    byte_end: Address,
    first_mask: u8,
    last_mask: u8,
    mut f: impl FnMut(Address, Address, u8, u8) -> core::ops::ControlFlow<B>,
) -> Option<B> {
    let grain = MMAPPER.granularity();
    let mut g_end = byte_end;
    while g_end > byte_start {
        let grain_base = (g_end - 1usize).align_down(grain);
        let g_start = grain_base.max(byte_start);
        let g_first = if g_start == byte_start {
            first_mask
        } else {
            0xff
        };
        let g_last = if g_end == byte_end { last_mask } else { 0xff };
        if let core::ops::ControlFlow::Break(val) = f(g_start, g_end, g_first, g_last) {
            return Some(val);
        }
        g_end = g_start;
    }
    None
}

#[inline(always)]
fn try_visit_masked_byte<T>(
    byte_idx: usize,
    mut b: u8,
    visit: &mut impl FnMut(usize, u8) -> core::ops::ControlFlow<T>,
) -> core::ops::ControlFlow<T> {
    while b != 0 {
        let bit = b.trailing_zeros() as u8;
        visit(byte_idx, bit)?;
        b &= b - 1;
    }
    core::ops::ControlFlow::Continue(())
}

#[inline(always)]
fn try_scan_non_zero_bits_in_slice<T>(
    slice: &[u8],
    base_align_rem: usize,
    first_mask: u8,
    last_mask: u8,
    mut visit: impl FnMut(usize, u8) -> core::ops::ControlFlow<T>,
) -> core::ops::ControlFlow<T> {
    const W: usize = core::mem::size_of::<usize>();
    let len = slice.len();
    if len == 0 {
        return core::ops::ControlFlow::Continue(());
    }
    if len == 1 {
        return try_visit_masked_byte(0, slice[0] & first_mask & last_mask, &mut visit);
    }

    let start = if first_mask != 0xff {
        try_visit_masked_byte(0, slice[0] & first_mask, &mut visit)?;
        1
    } else {
        0
    };
    let end = if last_mask != 0xff { len - 1 } else { len };

    let rem_align = (base_align_rem + start) & (W - 1);
    let prefix_len = ((W - rem_align) & (W - 1)).min(end - start);
    let mid_start = start + prefix_len;

    for (i, &b) in slice[start..mid_start].iter().enumerate() {
        try_visit_masked_byte(start + i, b, &mut visit)?;
    }

    let mid = &slice[mid_start..end];
    let chunks = mid.chunks_exact(W);
    let rem = chunks.remainder();
    let rem_start = mid_start + mid.len() - rem.len();
    for (chunk_idx, chunk) in chunks.enumerate() {
        let mut word = usize::from_le_bytes(chunk.try_into().unwrap());
        let chunk_base = mid_start + chunk_idx * W;
        while word != 0 {
            let bit = word.trailing_zeros() as usize;
            visit(chunk_base + (bit >> 3), (bit & 7) as u8)?;
            word &= word - 1;
        }
    }
    for (i, &b) in rem.iter().enumerate() {
        try_visit_masked_byte(rem_start + i, b, &mut visit)?;
    }

    if last_mask != 0xff {
        try_visit_masked_byte(len - 1, slice[len - 1] & last_mask, &mut visit)?;
    }
    core::ops::ControlFlow::Continue(())
}

/// Pure-safe forward bit search over `slice`, returning `(byte_offset, bit_in_byte)` of the lowest
/// set bit after masking `slice[0]` with `first_mask` and `slice[len - 1]` with `last_mask`.
#[inline]
pub(crate) fn find_first_non_zero_bit_in_slice(
    slice: &[u8],
    base_align_rem: usize,
    first_mask: u8,
    last_mask: u8,
) -> Option<(usize, u8)> {
    try_scan_non_zero_bits_in_slice(slice, base_align_rem, first_mask, last_mask, |byte, bit| {
        core::ops::ControlFlow::Break((byte, bit))
    })
    .break_value()
}

/// Pure-safe backward bit search over `slice`, returning `(byte_offset, bit_in_byte)` of the
/// highest set bit after masking `slice[0]` with `first_mask` and `slice[len - 1]` with `last_mask`.
#[inline]
pub(crate) fn find_last_non_zero_bit_in_slice(
    slice: &[u8],
    base_align_rem: usize,
    first_mask: u8,
    last_mask: u8,
) -> Option<(usize, u8)> {
    const W: usize = core::mem::size_of::<usize>();
    let len = slice.len();
    if len == 0 {
        return None;
    }
    if len == 1 {
        let b = slice[0] & first_mask & last_mask;
        return (b != 0).then(|| (0, 7 - b.leading_zeros() as u8));
    }

    let end = if last_mask != 0xff {
        let blast = slice[len - 1] & last_mask;
        if blast != 0 {
            return Some((len - 1, 7 - blast.leading_zeros() as u8));
        }
        len - 1
    } else {
        len
    };
    let start = if first_mask != 0xff { 1 } else { 0 };

    let end_align_rem = (base_align_rem + end) & (W - 1);
    let suffix_len = end_align_rem.min(end - start);
    let mid_end = end - suffix_len;

    for (i, &b) in slice[mid_end..end].iter().enumerate().rev() {
        if b != 0 {
            return Some((mid_end + i, 7 - b.leading_zeros() as u8));
        }
    }

    let mid = &slice[start..mid_end];
    let rchunks = mid.rchunks_exact(W);
    let rem = rchunks.remainder();
    for (rev_idx, chunk) in rchunks.enumerate() {
        let word = usize::from_le_bytes(chunk.try_into().unwrap());
        if word != 0 {
            let bit = (usize::BITS - 1 - word.leading_zeros()) as usize;
            let chunk_start = mid_end - (rev_idx + 1) * W;
            return Some((chunk_start + (bit >> 3), (bit & 7) as u8));
        }
    }
    for (i, &b) in rem.iter().enumerate().rev() {
        if b != 0 {
            return Some((start + i, 7 - b.leading_zeros() as u8));
        }
    }

    if first_mask != 0xff {
        let b0 = slice[0] & first_mask;
        if b0 != 0 {
            return Some((0, 7 - b0.leading_zeros() as u8));
        }
    }
    None
}

#[inline(always)]
fn visit_masked_byte(byte_idx: usize, mut b: u8, visit: &mut impl FnMut(usize, u8)) {
    while b != 0 {
        let bit = b.trailing_zeros() as u8;
        visit(byte_idx, bit);
        b &= b - 1;
    }
}

/// Pure-safe forward bit scan over `slice`, invoking `visit(byte_offset, bit_in_byte)` for every
/// non-zero bit after masking `slice[0]` with `first_mask` and `slice[len - 1]` with `last_mask`.
#[inline]
pub(super) fn scan_non_zero_bits_in_slice(
    slice: &[u8],
    base_align_rem: usize,
    first_mask: u8,
    last_mask: u8,
    mut visit: impl FnMut(usize, u8),
) {
    let _ = try_scan_non_zero_bits_in_slice(
        slice,
        base_align_rem,
        first_mask,
        last_mask,
        |byte, bit| {
            visit(byte, bit);
            core::ops::ControlFlow::Continue::<()>(())
        },
    );
}

#[inline(always)]
fn load_masked_atomic_byte(atom: &core::sync::atomic::AtomicU8, mask: u8) -> Option<u8> {
    use core::sync::atomic::Ordering;
    if atom.load(Ordering::Relaxed) & mask != 0 {
        let b = atom.load(Ordering::Acquire) & mask;
        if b != 0 {
            return Some(b);
        }
    }
    None
}

#[inline(always)]
fn chunk8_any_relaxed(chunk: &[core::sync::atomic::AtomicU8]) -> bool {
    use core::sync::atomic::Ordering;
    (chunk[0].load(Ordering::Relaxed)
        | chunk[1].load(Ordering::Relaxed)
        | chunk[2].load(Ordering::Relaxed)
        | chunk[3].load(Ordering::Relaxed)
        | chunk[4].load(Ordering::Relaxed)
        | chunk[5].load(Ordering::Relaxed)
        | chunk[6].load(Ordering::Relaxed)
        | chunk[7].load(Ordering::Relaxed))
        != 0
}

/// Pure-safe single-pass forward atomic scan over `&[AtomicU8]`, fast-skipping 8-byte zero runs
/// via `Ordering::Relaxed` `AtomicU8` loads and loading each individual non-zero byte with
/// `Ordering::Acquire` right before visiting its set bits so re-entrant callbacks mutating later
/// bytes via `AtomicU8` are observed cleanly without mixed-width atomic UB.
#[inline]
pub(crate) fn scan_non_zero_atomic_bits_in_slice(
    slice: &[core::sync::atomic::AtomicU8],
    first_mask: u8,
    last_mask: u8,
    mut visit: impl FnMut(usize, u8),
) {
    let len = slice.len();
    if len == 0 {
        return;
    }
    if len == 1 {
        if let Some(b) = load_masked_atomic_byte(&slice[0], first_mask & last_mask) {
            visit_masked_byte(0, b, &mut visit);
        }
        return;
    }
    let start = if first_mask != 0xff {
        if let Some(b) = load_masked_atomic_byte(&slice[0], first_mask) {
            visit_masked_byte(0, b, &mut visit);
        }
        1
    } else {
        0
    };
    let end = if last_mask != 0xff { len - 1 } else { len };
    let mid = &slice[start..end];
    let chunks = mid.chunks_exact(8);
    let rem = chunks.remainder();
    let rem_start = start + mid.len() - rem.len();
    for (chunk_idx, chunk) in chunks.enumerate() {
        if chunk8_any_relaxed(chunk) {
            let base = start + chunk_idx * 8;
            for (i, atom) in chunk.iter().enumerate() {
                if let Some(b) = load_masked_atomic_byte(atom, 0xff) {
                    visit_masked_byte(base + i, b, &mut visit);
                }
            }
        }
    }
    for (i, atom) in rem.iter().enumerate() {
        if let Some(b) = load_masked_atomic_byte(atom, 0xff) {
            visit_masked_byte(rem_start + i, b, &mut visit);
        }
    }
    if last_mask != 0xff {
        if let Some(b) = load_masked_atomic_byte(&slice[len - 1], last_mask) {
            visit_masked_byte(len - 1, b, &mut visit);
        }
    }
}

/// Pure-safe backward atomic search over `&[AtomicU8]`, fast-skipping 8-byte zero runs in reverse
/// with `Ordering::Relaxed` `AtomicU8` loads (never mixing atomic widths) and returning
/// `(byte_offset, masked_acquire_byte)` for the highest-address non-zero byte so the caller can
/// drop `slice` before invoking any callbacks or object-model queries.
#[inline]
pub(crate) fn find_last_non_zero_atomic_byte(
    slice: &[core::sync::atomic::AtomicU8],
    first_mask: u8,
    last_mask: u8,
) -> Option<(usize, u8)> {
    let len = slice.len();
    if len == 0 {
        return None;
    }
    if len == 1 {
        return load_masked_atomic_byte(&slice[0], first_mask & last_mask).map(|b| (0, b));
    }
    let end = if last_mask != 0xff {
        if let Some(b) = load_masked_atomic_byte(&slice[len - 1], last_mask) {
            return Some((len - 1, b));
        }
        len - 1
    } else {
        len
    };
    let start = if first_mask != 0xff { 1 } else { 0 };
    let mid = &slice[start..end];
    let rchunks = mid.rchunks_exact(8);
    let rem = rchunks.remainder();
    for (rev_idx, chunk) in rchunks.enumerate() {
        if chunk8_any_relaxed(chunk) {
            let chunk_start = end - (rev_idx + 1) * 8;
            for (i, atom) in chunk.iter().enumerate().rev() {
                if let Some(b) = load_masked_atomic_byte(atom, 0xff) {
                    return Some((chunk_start + i, b));
                }
            }
        }
    }
    for (i, atom) in rem.iter().enumerate().rev() {
        if let Some(b) = load_masked_atomic_byte(atom, 0xff) {
            return Some((start + i, b));
        }
    }
    if first_mask != 0xff {
        if let Some(b) = load_masked_atomic_byte(&slice[0], first_mask) {
            return Some((0, b));
        }
    }
    None
}

/// Scan `[meta_start, meta_end)` and invoke `visit_bit(byte_addr, bit_in_byte)` for every
/// non-zero bit in the side-metadata range (retained for `test_private` / `bscan` benchmarks).
///
/// # Safety
/// `[meta_start, meta_end)` must be non-null (when non-empty), mapped, initialized side-metadata
/// memory (`meta_end - meta_start <= isize::MAX`) with no concurrent writes or `&mut` aliases.
pub unsafe fn scan_non_zero_bits_in_metadata_bytes(
    meta_start: Address,
    meta_end: Address,
    visit_bit: &mut impl FnMut(Address, BitOffset),
) {
    if meta_start >= meta_end {
        return;
    }
    let len = meta_end - meta_start;
    assert!(!meta_start.is_zero() && len <= isize::MAX as usize);
    let align_rem = meta_start.as_usize() & (core::mem::size_of::<usize>() - 1);
    // SAFETY: Non-null `meta_start` and `len <= isize::MAX` verified above (`u8` has align 1);
    // caller guarantees `[meta_start, meta_end)` is mapped, initialized, and quiescent.
    let slice = unsafe { core::slice::from_raw_parts(meta_start.to_ptr::<u8>(), len) };
    scan_non_zero_bits_in_slice(slice, align_rem, 0xff, 0xff, |byte_off, bit| {
        visit_bit(meta_start + byte_off, bit);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::metadata::side_metadata::*;

    fn should_skip_spec_on_this_target(spec: &SideMetadataSpec) -> bool {
        use layout::LOG_GLOBAL_SIDE_METADATA_WORST_CASE_RATIO;
        log_data_meta_ratio(spec) < LOG_GLOBAL_SIDE_METADATA_WORST_CASE_RATIO
    }

    fn test_round_trip_conversion(spec: &SideMetadataSpec, raw_addrs: &[usize]) {
        if should_skip_spec_on_this_target(spec) {
            eprintln!(
                "Skipping {} on this target: spec ratio is outside global side metadata worst-case bound",
                spec.name
            );
            return;
        }

        core_test_initialize_side_metadata();

        for &raw in raw_addrs {
            // SAFETY: Synthetic aligned test addresses used solely for pure address-translation
            // arithmetic and never dereferenced.
            let addr = unsafe { Address::from_usize(raw) };

            // This is an aligned address. When we do roundtrip conversion, we will get back the original address.
            assert!(addr.is_aligned_to(1 << spec.log_bytes_in_region));
            let meta_addr = address_to_contiguous_meta_address(spec, addr);
            let shift = meta_byte_lshift(spec, addr);
            assert_eq!(
                contiguous_meta_address_to_address(spec, meta_addr, shift),
                addr
            );

            // This is an unaligned address. When we do roundtrip conversion, we will get the aligned address.
            let next_addr = addr + 1usize;
            let meta_addr = address_to_contiguous_meta_address(spec, next_addr);
            let shift = meta_byte_lshift(spec, next_addr);
            assert_eq!(
                contiguous_meta_address_to_address(spec, meta_addr, shift),
                addr
            );
        }
    }

    const TEST_ADDRESS_8B_REGION: [usize; 8] = [
        0x8000_0000,
        0x8000_0008,
        0x8000_0010,
        0x8000_0018,
        0x8000_0020,
        0x8001_0000,
        0x8001_0008,
        0xd000_0000,
    ];

    #[test]
    fn test_contiguous_metadata_conversion_0_3() {
        let spec = SideMetadataSpec {
            name: "ContiguousMetadataTestSpec",
            is_global: true,
            offset: 0,
            log_num_of_bits: 0,
            log_bytes_in_region: 3,
        };

        test_round_trip_conversion(&spec, &TEST_ADDRESS_8B_REGION);
    }

    #[test]
    fn test_contiguous_metadata_conversion_1_3() {
        let spec = SideMetadataSpec {
            name: "ContiguousMetadataTestSpec",
            is_global: true,
            offset: 0,
            log_num_of_bits: 1,
            log_bytes_in_region: 3,
        };

        test_round_trip_conversion(&spec, &TEST_ADDRESS_8B_REGION);
    }

    #[test]
    fn test_contiguous_metadata_conversion_4_3() {
        let spec = SideMetadataSpec {
            name: "ContiguousMetadataTestSpec",
            is_global: true,
            offset: 0,
            log_num_of_bits: 4,
            log_bytes_in_region: 3,
        };

        test_round_trip_conversion(&spec, &TEST_ADDRESS_8B_REGION);
    }

    #[test]
    fn test_contiguous_metadata_conversion_5_3() {
        let spec = SideMetadataSpec {
            name: "ContiguousMetadataTestSpec",
            is_global: true,
            offset: 0,
            log_num_of_bits: 5,
            log_bytes_in_region: 3,
        };

        test_round_trip_conversion(&spec, &TEST_ADDRESS_8B_REGION);
    }

    const TEST_ADDRESS_4KB_REGION: [usize; 8] = [
        0x8000_0000,
        0x8000_1000,
        0x8000_2000,
        0x8000_3000,
        0x8000_4000,
        0x8001_0000,
        0x8001_1000,
        0xd000_0000,
    ];

    #[test]
    fn test_contiguous_metadata_conversion_0_12() {
        let spec = SideMetadataSpec {
            name: "ContiguousMetadataTestSpec",
            is_global: true,
            offset: 0,
            log_num_of_bits: 0,
            log_bytes_in_region: 12, // 4K
        };

        test_round_trip_conversion(&spec, &TEST_ADDRESS_4KB_REGION);
    }

    #[test]
    fn test_safe_slice_bit_kernels() {
        let mut buf = [0u8; 32];
        buf[0] = 0b0010_0101;
        buf[11] = 0b0000_1000;
        buf[29] = 0b0100_0000;

        for align_rem in 0..core::mem::size_of::<usize>() {
            assert_eq!(
                find_first_non_zero_bit_in_slice(&buf[..1], align_rem, 0xff, 0b0000_0001),
                Some((0, 0))
            );
            assert_eq!(
                find_last_non_zero_bit_in_slice(&buf[..1], align_rem, 0xff, 0b0000_0111),
                Some((0, 2))
            );
            assert_eq!(
                find_last_non_zero_bit_in_slice(&buf[..1], align_rem, 0xff, 0xff),
                Some((0, 5))
            );
            assert_eq!(
                find_first_non_zero_bit_in_slice(&[0u8], align_rem, 0xff, 0b0000_0001),
                None
            );

            // Mask out bits 0..=2 of byte 0 and bit 6 of byte 29:
            assert_eq!(
                find_first_non_zero_bit_in_slice(&buf[..30], align_rem, 0b1111_1000, 0b0011_1111),
                Some((0, 5))
            );
            assert_eq!(
                find_last_non_zero_bit_in_slice(&buf[..30], align_rem, 0b1111_1000, 0b0011_1111),
                Some((11, 3))
            );

            let mut visited = Vec::new();
            scan_non_zero_bits_in_slice(&buf, align_rem, 0b1111_1100, 0xff, |b, bit| {
                visited.push((b, bit));
            });
            assert_eq!(visited, vec![(0, 2), (0, 5), (11, 3), (29, 6)]);
        }

        let atoms: [core::sync::atomic::AtomicU8; 32] =
            core::array::from_fn(|i| core::sync::atomic::AtomicU8::new(buf[i]));
        let scan_atomic = |slice: &[core::sync::atomic::AtomicU8], fm: u8, lm: u8| {
            let mut out = Vec::new();
            scan_non_zero_atomic_bits_in_slice(slice, fm, lm, |b, bit| out.push((b, bit)));
            out
        };
        assert!(scan_atomic(&[], 0xff, 0xff).is_empty());
        assert_eq!(find_last_non_zero_atomic_byte(&[], 0xff, 0xff), None);
        assert_eq!(
            scan_atomic(&atoms[..1], 0xff, 0b0000_0111),
            vec![(0, 0), (0, 2)]
        );
        assert_eq!(
            find_last_non_zero_atomic_byte(&atoms[..1], 0xff, 0b0000_0111),
            Some((0, 0b0000_0101))
        );
        assert_eq!(
            find_last_non_zero_atomic_byte(&atoms[..1], 0b1000_0000, 0xff),
            None
        );
        assert_eq!(
            scan_atomic(&atoms[..30], 0b1111_1000, 0b0011_1111),
            vec![(0, 5), (11, 3)]
        );
        assert_eq!(
            scan_atomic(&atoms, 0b1111_1100, 0xff),
            vec![(0, 2), (0, 5), (11, 3), (29, 6)]
        );
        assert_eq!(
            find_last_non_zero_atomic_byte(&atoms[..30], 0b1111_1000, 0b0011_1111),
            Some((11, 0b0000_1000))
        );
        assert_eq!(
            find_last_non_zero_atomic_byte(&atoms[..30], 0b1111_1000, 0xff),
            Some((29, 0b0100_0000))
        );
        assert_eq!(
            find_last_non_zero_atomic_byte(&atoms[..11], 0b0010_0100, 0xff),
            Some((0, 0b0010_0100))
        );
    }

    #[test]
    fn test_align_metadata_address() {
        #[repr(align(4096))]
        struct Aligned4K([u8; 2]);

        let backing = Aligned4K([0; 2]);
        let addr_1000 = Address::from_ptr(&backing.0[0]);
        let addr_1001 = addr_1000 + 1usize;

        let create_spec = |log_num_of_bits: usize| SideMetadataSpec {
            name: "AlignMetadataBitTestSpec",
            is_global: true,
            offset: 0,
            log_num_of_bits,
            log_bytes_in_region: 3,
        };

        let metadata_2bits = create_spec(1);
        for bit in 0..8u8 {
            assert_eq!(
                align_metadata_address(&metadata_2bits, addr_1000, bit),
                (addr_1000, bit & !0b1)
            );
        }

        let metadata_4bits = create_spec(2);
        for bit in 0..8u8 {
            assert_eq!(
                align_metadata_address(&metadata_4bits, addr_1000, bit),
                (addr_1000, bit & !0b11)
            );
        }

        let metadata_8bits = create_spec(3);
        for bit in 0..8u8 {
            assert_eq!(
                align_metadata_address(&metadata_8bits, addr_1000, bit),
                (addr_1000, 0)
            );
        }

        let metadata_16bits = create_spec(4);
        for addr in [addr_1000, addr_1001] {
            for bit in 0..8u8 {
                assert_eq!(
                    align_metadata_address(&metadata_16bits, addr, bit),
                    (addr_1000, 0)
                );
            }
        }
    }
}
