#![deny(unsafe_op_in_unsafe_fn)]

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

#[allow(dead_code)]
pub(super) const fn metadata_address_range_size(metadata_spec: &SideMetadataSpec) -> usize {
    1usize << (VMLayout::LOG_ARCH_ADDRESS_SPACE - log_data_meta_ratio(metadata_spec))
}

pub(crate) fn meta_byte_lshift(metadata_spec: &SideMetadataSpec, data_addr: Address) -> u8 {
    let bits_num_log = metadata_spec.log_num_of_bits as i32;
    if bits_num_log >= 3 {
        return 0;
    }
    let rem_shift = BITS_IN_WORD as i32 - ((LOG_BITS_IN_BYTE as i32) - bits_num_log);
    ((((data_addr >> metadata_spec.log_bytes_in_region) << rem_shift) >> rem_shift) << bits_num_log)
        as u8
}

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

/// If `grain_start.is_mapped()` holds for a non-empty sub-range `[grain_start, grain_end)` within a
/// single `MMAPPER.granularity()` grain, project `[grain_start, grain_end)` as `&[u8]` for `f`.
///
/// # Safety
/// * `[grain_start, grain_end)` must be non-empty and lie within a single `MMAPPER.granularity()`
///   grain (`grain_start.align_down(MMAPPER.granularity()) == (grain_end - 1usize).align_down(MMAPPER.granularity())`).
/// * Whenever `grain_start.is_mapped()` returns `true`, all bytes in `[grain_start, grain_end)`
///   must be initialized side-metadata memory with no concurrent writes, atomic modifications, or
///   live `&mut` aliases for the duration of `f`.
#[inline]
pub(super) unsafe fn with_mapped_meta_slice<R>(
    grain_start: Address,
    grain_end: Address,
    f: impl FnOnce(&[u8]) -> R,
) -> Option<R> {
    debug_assert!(grain_start < grain_end);
    debug_assert_eq!(
        grain_start.align_down(MMAPPER.granularity()),
        (grain_end - 1usize).align_down(MMAPPER.granularity())
    );
    if grain_start.is_zero() || !grain_start.is_mapped() {
        return None;
    }
    let len = grain_end - grain_start;
    assert!(len <= isize::MAX as usize);
    // SAFETY: `grain_start` is non-zero, `grain_start.is_mapped()` verified the entire enclosing
    // `MMAPPER.granularity()` grain is mapped (`u8` has alignment 1 and `len <= isize::MAX`), and
    // caller guarantees initialized side-metadata bytes without concurrent mutations or `&mut`
    // aliases for the duration of `f`.
    let slice = unsafe { core::slice::from_raw_parts(grain_start.to_ptr::<u8>(), len) };
    Some(f(slice))
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
    const W: usize = core::mem::size_of::<usize>();
    let len = slice.len();
    if len == 0 {
        return None;
    }
    if len == 1 {
        let b = slice[0] & first_mask & last_mask;
        return (b != 0).then(|| (0, b.trailing_zeros() as u8));
    }

    let start = if first_mask != 0xff {
        let b0 = slice[0] & first_mask;
        if b0 != 0 {
            return Some((0, b0.trailing_zeros() as u8));
        }
        1
    } else {
        0
    };
    let end = if last_mask != 0xff { len - 1 } else { len };

    let rem_align = (base_align_rem + start) & (W - 1);
    let prefix_len = ((W - rem_align) & (W - 1)).min(end - start);
    let mid_start = start + prefix_len;

    for (i, &b) in slice[start..mid_start].iter().enumerate() {
        if b != 0 {
            return Some((start + i, b.trailing_zeros() as u8));
        }
    }

    let mid = &slice[mid_start..end];
    let chunks = mid.chunks_exact(W);
    let rem = chunks.remainder();
    let rem_start = mid_start + mid.len() - rem.len();
    for (chunk_idx, chunk) in chunks.enumerate() {
        let word = usize::from_le_bytes(chunk.try_into().unwrap());
        if word != 0 {
            let bit = word.trailing_zeros() as usize;
            return Some((mid_start + chunk_idx * W + (bit >> 3), (bit & 7) as u8));
        }
    }
    for (i, &b) in rem.iter().enumerate() {
        if b != 0 {
            return Some((rem_start + i, b.trailing_zeros() as u8));
        }
    }

    if last_mask != 0xff {
        let blast = slice[len - 1] & last_mask;
        if blast != 0 {
            return Some((len - 1, blast.trailing_zeros() as u8));
        }
    }
    None
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
    const W: usize = core::mem::size_of::<usize>();
    let len = slice.len();
    if len == 0 {
        return;
    }
    if len == 1 {
        visit_masked_byte(0, slice[0] & first_mask & last_mask, &mut visit);
        return;
    }

    let start = if first_mask != 0xff {
        visit_masked_byte(0, slice[0] & first_mask, &mut visit);
        1
    } else {
        0
    };
    let end = if last_mask != 0xff { len - 1 } else { len };

    let rem_align = (base_align_rem + start) & (W - 1);
    let prefix_len = ((W - rem_align) & (W - 1)).min(end - start);
    let mid_start = start + prefix_len;

    for (i, &b) in slice[start..mid_start].iter().enumerate() {
        visit_masked_byte(start + i, b, &mut visit);
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
            visit(chunk_base + (bit >> 3), (bit & 7) as u8);
            word &= word - 1;
        }
    }
    for (i, &b) in rem.iter().enumerate() {
        visit_masked_byte(rem_start + i, b, &mut visit);
    }

    if last_mask != 0xff {
        visit_masked_byte(len - 1, slice[len - 1] & last_mask, &mut visit);
    }
}

/// Pure-safe forward atomic search over `&[AtomicU8]`, fast-skipping 8-byte zero runs with
/// `Ordering::Relaxed` `AtomicU8` loads (never mixing atomic widths) and returning
/// `(byte_offset, masked_acquire_byte)` for the first non-zero byte so the caller can drop `slice`
/// before invoking user callbacks.
#[inline]
pub(crate) fn find_first_non_zero_atomic_byte(
    slice: &[core::sync::atomic::AtomicU8],
    first_mask: u8,
    last_mask: u8,
) -> Option<(usize, u8)> {
    use core::sync::atomic::Ordering;
    let len = slice.len();
    if len == 0 {
        return None;
    }
    if len == 1 {
        let mask = first_mask & last_mask;
        if slice[0].load(Ordering::Relaxed) & mask != 0 {
            let b = slice[0].load(Ordering::Acquire) & mask;
            if b != 0 {
                return Some((0, b));
            }
        }
        return None;
    }

    let start = if first_mask != 0xff {
        if slice[0].load(Ordering::Relaxed) & first_mask != 0 {
            let b = slice[0].load(Ordering::Acquire) & first_mask;
            if b != 0 {
                return Some((0, b));
            }
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
        let any = chunk[0].load(Ordering::Relaxed)
            | chunk[1].load(Ordering::Relaxed)
            | chunk[2].load(Ordering::Relaxed)
            | chunk[3].load(Ordering::Relaxed)
            | chunk[4].load(Ordering::Relaxed)
            | chunk[5].load(Ordering::Relaxed)
            | chunk[6].load(Ordering::Relaxed)
            | chunk[7].load(Ordering::Relaxed);
        if any != 0 {
            let base = start + chunk_idx * 8;
            for (i, atom) in chunk.iter().enumerate() {
                if atom.load(Ordering::Relaxed) != 0 {
                    let b = atom.load(Ordering::Acquire);
                    if b != 0 {
                        return Some((base + i, b));
                    }
                }
            }
        }
    }
    for (i, atom) in rem.iter().enumerate() {
        if atom.load(Ordering::Relaxed) != 0 {
            let b = atom.load(Ordering::Acquire);
            if b != 0 {
                return Some((rem_start + i, b));
            }
        }
    }

    if last_mask != 0xff && slice[len - 1].load(Ordering::Relaxed) & last_mask != 0 {
        let b = slice[len - 1].load(Ordering::Acquire) & last_mask;
        if b != 0 {
            return Some((len - 1, b));
        }
    }
    None
}

/// Scan `[meta_start, meta_end)` and invoke `visit_bit(byte_or_word_addr, bit_offset)` for every
/// non-zero bit in the side-metadata range (retained for `test_private` / `bscan` benchmarks).
///
/// # Safety
/// * `[meta_start, meta_end)` must be non-null (whenever `meta_start < meta_end`), mapped,
///   readable, and initialized side-metadata memory (`meta_end - meta_start <= isize::MAX`).
/// * Neither another thread nor `visit_bit` may concurrently write to or atomically modify any
///   byte in `[meta_start, meta_end)` for the duration of this scan, and no live `&mut` reference
///   may alias `[meta_start, meta_end)`.
pub unsafe fn scan_non_zero_bits_in_metadata_bytes(
    meta_start: Address,
    meta_end: Address,
    visit_bit: &mut impl FnMut(Address, BitOffset),
) {
    const W: usize = core::mem::size_of::<usize>();
    if meta_start >= meta_end {
        return;
    }
    let len = meta_end - meta_start;
    assert!(!meta_start.is_zero() && len <= isize::MAX as usize);
    // SAFETY: Non-null `meta_start` and `len <= isize::MAX` were verified above (`u8` has alignment
    // 1), and caller guarantees `[meta_start, meta_end)` is mapped, initialized, and quiescent
    // (including across `visit_bit`) without live `&mut` aliases.
    let slice = unsafe { core::slice::from_raw_parts(meta_start.to_ptr::<u8>(), len) };

    let rem_align = meta_start.as_usize() & (W - 1);
    let prefix_len = ((W - rem_align) & (W - 1)).min(len);
    for (i, &b) in slice[..prefix_len].iter().enumerate() {
        scan_non_zero_bits_in_metadata_word(meta_start + i, b as usize, visit_bit);
    }

    let mid = &slice[prefix_len..];
    let chunks = mid.chunks_exact(W);
    let rem = chunks.remainder();
    let rem_start = prefix_len + mid.len() - rem.len();
    for (chunk_idx, chunk) in chunks.enumerate() {
        let word = usize::from_ne_bytes(chunk.try_into().unwrap());
        scan_non_zero_bits_in_metadata_word(
            meta_start + prefix_len + chunk_idx * W,
            word,
            visit_bit,
        );
    }
    for (i, &b) in rem.iter().enumerate() {
        scan_non_zero_bits_in_metadata_word(meta_start + rem_start + i, b as usize, visit_bit);
    }
}

fn scan_non_zero_bits_in_metadata_word(
    meta_addr: Address,
    mut word: usize,
    visit_bit: &mut impl FnMut(Address, BitOffset),
) {
    while word != 0 {
        let bit = word.trailing_zeros();
        visit_bit(meta_addr, bit as u8);
        word &= word - 1;
    }
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
