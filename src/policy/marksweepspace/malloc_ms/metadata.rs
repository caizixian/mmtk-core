#![deny(unsafe_op_in_unsafe_fn)]

use crate::util::metadata::side_metadata::SideMetadataSpec;
use crate::util::metadata::vo_bit;
use crate::util::Address;
use crate::util::ObjectReference;
use crate::vm::{ObjectModel, VMBinding};
use std::sync::atomic::Ordering;

/// Metadata spec for the active page byte
///
/// The active page metadata is used to accurately track the total number of pages that have
/// been reserved by `malloc()`.
///
/// We use a byte instead of a bit to avoid synchronization costs, i.e. to avoid
/// the case where two threads try to update different bits in the same byte at
/// the same time
pub(crate) const ACTIVE_PAGE_METADATA_SPEC: SideMetadataSpec =
    crate::util::metadata::side_metadata::spec_defs::MALLOC_MS_ACTIVE_PAGE;

pub(crate) const OFFSET_MALLOC_METADATA_SPEC: SideMetadataSpec =
    crate::util::metadata::side_metadata::spec_defs::MS_OFFSET_MALLOC;

pub(super) fn is_marked<VM: VMBinding>(object: ObjectReference, ordering: Ordering) -> bool {
    VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.load_atomic::<VM, u8>(object, None, ordering) == 1
}

/// Set the page mark from 0 to 1. Return true if we set it successfully in this call.
pub(super) fn compare_exchange_set_page_mark(page_addr: Address) -> bool {
    // The spec has 1 byte per each page. So it won't be the case that other threads may race and access other bits for the spec.
    // If the compare-exchange fails, we know the byte was set to 1 before this call.
    ACTIVE_PAGE_METADATA_SPEC
        .compare_exchange_atomic::<u8>(page_addr, 0, 1, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

#[allow(unused)]
pub(super) fn is_page_marked(page_addr: Address) -> bool {
    ACTIVE_PAGE_METADATA_SPEC.load_atomic::<u8>(page_addr, Ordering::SeqCst) == 1
}

pub(super) fn set_vo_bit(object: ObjectReference) {
    vo_bit::set_vo_bit(object);
}

pub(super) fn set_mark_bit<VM: VMBinding>(object: ObjectReference, ordering: Ordering) {
    VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.store_atomic::<VM, u8>(object, 1, None, ordering);
}

#[allow(unused)]
pub(super) fn unset_vo_bit(object: ObjectReference) {
    vo_bit::unset_vo_bit(object);
}

#[allow(unused)]
pub(super) fn set_page_mark(page_addr: Address) {
    ACTIVE_PAGE_METADATA_SPEC.store_atomic::<u8>(page_addr, 1, Ordering::SeqCst);
}

/// Atomically check whether the allocation starting at `address` (object start) was an offset malloc.
///
/// Uses `Ordering::Relaxed` atomic access because an object whose `ObjectReference` lies near the
/// start of chunk `C + 1` can have `object_start` near the end of chunk `C`, sharing an
/// `OFFSET_MALLOC_METADATA_SPEC` byte with objects swept concurrently by chunk `C`'s worker.
#[inline(always)]
pub(super) fn is_offset_malloc(address: Address) -> bool {
    OFFSET_MALLOC_METADATA_SPEC.load_atomic::<u8>(address, Ordering::Relaxed) == 1
}

/// Set the offset bit for the allocation. The argument address should be the allocation address (object start).
#[inline(always)]
pub(super) fn set_offset_malloc_bit(address: Address) {
    OFFSET_MALLOC_METADATA_SPEC.store_atomic::<u8>(address, 1, Ordering::SeqCst);
}

/// Atomically unset the offset bit for the allocation starting at `address` (object start).
///
/// Kept strictly atomic so two `MSSweepChunk` workers cannot race or lose updates on a shared
/// `OFFSET_MALLOC_METADATA_SPEC` byte when a cross-chunk allocation straddles a chunk boundary.
#[inline(always)]
pub(super) fn unset_offset_malloc_bit(address: Address) {
    OFFSET_MALLOC_METADATA_SPEC.store_atomic::<u8>(address, 0, Ordering::Relaxed);
}

/// Reinterpret an 8-byte-aligned side-metadata byte slice (whose length is a multiple of 16 bytes)
/// as 128-bit `[u64; 2]` pairs using 100% safe `bytemuck::cast_slice`.
///
/// Using `[u64; 2]` (`align_of::<[u64; 2]>() == 8 == SIDE_METADATA_SPEC_ALIGN`) avoids the 16-byte
/// alignment hazard of `u128` on Rust >= 1.77 while compiling `[pair == [0, 0]]` and pair-XOR
/// checks to identical 128-bit loads.
#[inline(always)]
pub(super) fn as_u64_pairs(bytes: &[u8]) -> &[[u64; 2]] {
    bytemuck::cast_slice::<u8, [u64; 2]>(bytes)
}

#[cfg(test)]
mod tests {
    use super::as_u64_pairs;

    #[repr(align(8))]
    struct AlignedBytes<const N: usize>([u8; N]);

    #[test]
    fn test_as_u64_pairs_zero_and_xor_scan() {
        // Test at an 8-byte-aligned (potentially non-16-byte-aligned) offset by slicing at byte 8
        // inside a 40-byte 8-aligned buffer.
        let mut buf = AlignedBytes([0u8; 40]);
        let slice = &mut buf.0[8..40];
        assert_eq!(as_u64_pairs(slice), &[[0, 0], [0, 0]]);

        slice[19] = 0b0010_0000;
        let pairs = as_u64_pairs(slice);
        assert_eq!(pairs[0], [0, 0]);
        assert_ne!(pairs[1], [0, 0]);

        let mut mark_buf = AlignedBytes([0u8; 40]);
        let mark_slice = &mut mark_buf.0[8..40];
        mark_slice[19] = 0b0010_0000;
        let vo_pairs = as_u64_pairs(slice);
        let mark_pairs = as_u64_pairs(mark_slice);
        assert_eq!(
            (vo_pairs[1][0] ^ mark_pairs[1][0]) | (vo_pairs[1][1] ^ mark_pairs[1][1]),
            0
        );
    }
}
