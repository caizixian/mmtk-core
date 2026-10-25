use crate::util::metadata::side_metadata;
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
// XXX: This metadata spec is currently unused as we need to add a performant way to calculate
// how many pages are active in this metadata spec. Explore SIMD vectorization with 8-bit integers
pub(crate) const ACTIVE_PAGE_METADATA_SPEC: SideMetadataSpec =
    crate::util::metadata::side_metadata::spec_defs::MALLOC_MS_ACTIVE_PAGE;

pub(crate) const OFFSET_MALLOC_METADATA_SPEC: SideMetadataSpec =
    crate::util::metadata::side_metadata::spec_defs::MS_OFFSET_MALLOC;

pub fn is_marked<VM: VMBinding>(object: ObjectReference, ordering: Ordering) -> bool {
    VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.load_atomic::<VM, u8>(object, None, ordering) == 1
}

/// Non-atomically check if `object`'s mark bit is set.
///
/// # Safety
/// - `object` must be a valid `MallocSpace` object whose mark-bit metadata (in-header or mapped
///   side metadata for `object.to_raw_address()`) is allocated/mapped and initialized.
/// - No other thread may concurrently modify the metadata byte/word containing `object`'s mark bit.
pub unsafe fn is_marked_unsafe<VM: VMBinding>(object: ObjectReference) -> bool {
    // SAFETY: Caller guarantees `object` has valid/mapped mark-bit metadata and that no other
    // thread concurrently modifies the underlying metadata byte/word.
    unsafe { VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.load::<VM, u8>(object, None) == 1 }
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

/// Non-atomically check if the active-page byte for `page_addr` is set.
///
/// # Safety
/// - `ACTIVE_PAGE_METADATA_SPEC` side metadata must be mapped for `page_addr`.
/// - No other thread may concurrently modify the active-page metadata byte for `page_addr`.
#[allow(unused)]
pub(super) unsafe fn is_page_marked_unsafe(page_addr: Address) -> bool {
    // SAFETY: Caller guarantees `ACTIVE_PAGE_METADATA_SPEC` is mapped for `page_addr` and no other
    // thread concurrently writes this page's metadata byte.
    unsafe { ACTIVE_PAGE_METADATA_SPEC.load::<u8>(page_addr) == 1 }
}

pub fn set_vo_bit(object: ObjectReference) {
    vo_bit::set_vo_bit(object);
}

pub fn set_mark_bit<VM: VMBinding>(object: ObjectReference, ordering: Ordering) {
    VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.store_atomic::<VM, u8>(object, 1, None, ordering);
}

#[allow(unused)]
pub fn unset_vo_bit(object: ObjectReference) {
    vo_bit::unset_vo_bit(object);
}

#[allow(unused)]
pub(super) fn set_page_mark(page_addr: Address) {
    ACTIVE_PAGE_METADATA_SPEC.store_atomic::<u8>(page_addr, 1, Ordering::SeqCst);
}

/// Check whether the allocation starting at `address` (object start) was an offset malloc.
///
/// Note: Uses a `Relaxed` atomic load (which compiles to a plain load on x86_64/aarch64) rather
/// than a non-atomic load because when `ObjectReference` is offset past `object_start`, an object
/// whose `ObjectReference` lies at the start of chunk `C + 1` can have `object_start` near the end
/// of chunk `C`, sharing an `OFFSET_MALLOC_METADATA_SPEC` byte with objects swept concurrently by
/// chunk `C`'s worker.
///
/// # Safety
/// `OFFSET_MALLOC_METADATA_SPEC` side metadata must be mapped for `address`.
pub(super) unsafe fn is_offset_malloc_unsafe(address: Address) -> bool {
    OFFSET_MALLOC_METADATA_SPEC.load_atomic::<u8>(address, Ordering::Relaxed) == 1
}

/// Set the offset bit for the allocation. The argument address should be the allocation address (object start)
pub(super) fn set_offset_malloc_bit(address: Address) {
    OFFSET_MALLOC_METADATA_SPEC.store_atomic::<u8>(address, 1, Ordering::SeqCst);
}

/// Unset the offset bit for the allocation starting at `address` (object start).
///
/// Uses an atomic bit-clear so two `MSSweepChunk` workers cannot race or lose updates on a shared
/// `OFFSET_MALLOC_METADATA_SPEC` byte if `object_start` lies in the preceding chunk while
/// `ObjectReference` lies in the current chunk.
///
/// # Safety
/// `OFFSET_MALLOC_METADATA_SPEC` side metadata must be mapped for `address`.
pub(super) unsafe fn unset_offset_malloc_bit_unsafe(address: Address) {
    OFFSET_MALLOC_METADATA_SPEC.store_atomic::<u8>(address, 0, Ordering::Relaxed);
}

/// Non-atomically unset the VO bit for `object`.
///
/// # Safety
/// - `VO_BIT` side metadata must be mapped for `object.to_raw_address()`.
/// - No other thread may concurrently access the VO-bit metadata byte containing `object`
///   (e.g., called by the unique `MSSweepChunk` worker owning `object.to_raw_address()`'s chunk).
pub unsafe fn unset_vo_bit_unsafe(object: ObjectReference) {
    // SAFETY: Caller guarantees `VO_BIT` metadata for `object` is mapped and exclusively accessed
    // by the current thread.
    unsafe { vo_bit::unset_vo_bit_unsafe(object) };
}

/// Non-atomically unset the mark bit for `object`.
///
/// # Safety
/// - `object` must be a valid `MallocSpace` object whose mark-bit metadata (in-header or mapped
///   side metadata for `object.to_raw_address()`) is valid and mapped.
/// - No other thread may concurrently access the metadata byte/word containing `object`'s mark bit.
#[allow(unused)]
pub unsafe fn unset_mark_bit<VM: VMBinding>(object: ObjectReference) {
    // SAFETY: Caller guarantees `object` mark-bit metadata is valid/mapped and exclusively
    // accessed by the current thread.
    unsafe { VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.store::<VM, u8>(object, 0, None) };
}

/// Non-atomically clear the active-page byte for `page_addr`.
///
/// # Safety
/// - `ACTIVE_PAGE_METADATA_SPEC` side metadata must be mapped for `page_addr`.
/// - No other thread may concurrently access the active-page metadata byte for `page_addr`.
#[allow(unused)]
pub(super) unsafe fn unset_page_mark_unsafe(page_addr: Address) {
    // SAFETY: Caller guarantees `ACTIVE_PAGE_METADATA_SPEC` is mapped for `page_addr` and
    // exclusively accessed by this thread.
    unsafe { ACTIVE_PAGE_METADATA_SPEC.store::<u8>(page_addr, 0) }
}

/// Load 128 bits of side metadata starting at `data_addr`.
///
/// Uses `read_unaligned` because `align_of::<u128>()` is 16 bytes on Rust >= 1.77 (x86_64/aarch64),
/// whereas side-metadata spec base offsets are only guaranteed to be word/8-byte aligned (and
/// `read_unaligned::<u128>()` compiles to the same pair of 64-bit loads `mov`/`ldp` with zero
/// overhead).
///
/// # Safety
/// - `data_addr` must be aligned to `128 << metadata_spec.log_bytes_in_region` (so `data_addr` maps
///   to a byte boundary in `metadata_spec`) and at least `size_of::<u128>()` contiguous bytes of
///   side metadata starting at `address_to_meta_address(metadata_spec, data_addr)` must be mapped
///   and initialized.
/// - No other thread may concurrently write to those 16 metadata bytes.
pub(super) unsafe fn load128(metadata_spec: &SideMetadataSpec, data_addr: Address) -> u128 {
    debug_assert_eq!(metadata_spec.log_num_of_bits, 0);
    debug_assert!(data_addr.is_aligned_to(128usize << metadata_spec.log_bytes_in_region));
    let meta_addr = side_metadata::address_to_meta_address(metadata_spec, data_addr);

    #[cfg(all(debug_assertions, feature = "extreme_assertions"))]
    metadata_spec.assert_metadata_mapped(data_addr);

    // SAFETY: Caller guarantees 16 contiguous mapped, initialized metadata bytes at `meta_addr`
    // with no concurrent writers; `read_unaligned` has no alignment requirement beyond 1 byte.
    unsafe { meta_addr.to_ptr::<u128>().read_unaligned() }
}
