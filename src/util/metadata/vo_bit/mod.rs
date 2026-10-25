//! Valid object bit (VO bit)
//!
//! The valid object bit, or "VO bit" for short", is a global per-address metadata.  It is set at
//! the address of the `ObjectReference` of an object when the object is allocated, and cleared
//! when the object is determined to be dead by the GC.
//!
//! The VO bit metadata serves multiple purposes, including but not limited to:
//!
//! | purpose                                     | happens when                                  |
//! |---------------------------------------------|-----------------------------------------------|
//! | conservative stack scanning                 | stack scanning                                |
//! | conservative object scanning                | tracing                                       |
//! | supporting interior pointers                | tracing                                       |
//! | heap dumping (by tracing)                   | tracing                                       |
//! | heap dumping (by iteration)                 | before or after tracing                       |
//! | heap iteration (for GC algorithm)           | depending on algorithm                        |
//! | heap iteration (for VM API, e.g. JVM-TI)    | during mutator time                           |
//! | sanity checking                             | any time in GC                                |
//!
//! Among the listed purposes, conservative stack scanning and conservative objects scanning are
//! visible to the VM binding.  By default, if the "vo_bit" cargo feature is enabled, the VO bits
//! metadata will be available to the VM binding during stack scanning time.  The VM binding can
//! further require the VO bits to be available during tracing (for object scanning) by setting
//! [`crate::vm::ObjectModel::NEED_VO_BITS_DURING_TRACING`] to `true`.  mmtk-core does not
//! guarantee the VO bits are available to the VM binding during other time.
//!
//! Internally, mmtk-core will also make the VO bits available when necessary if mmtk-core needs to
//! implement features that needs VO bits.
//!
//! When the VO bits are available during tracing, if a plan uses evacuation to reclaim space, then
//! both the from-space copy and the to-space copy of an object will have the VO-bit set.
//!
//! *(Note: There are several reasons behind this semantics.  One reason is that a slot may be
//! visited multiple times during GC.  If a slot is visited twice, we will see the object reference
//! in the slot pointing to the from-space copy during the first visit, but pointing to the to-space
//! copy during the second visit.  We consider an object reference valid if it points to either the
//! from-space or the to-space copy.  If each slot is visited only once, and we see a slot happen to
//! hold a pointer into the to-space during its only visit, that must be a dangling pointer, and
//! error should be reported.  However, it is hard to guarantee each slot is only visited once
//! during tracing because both the VM and the GC algorithm may break this guarantee.  See:
//! [`crate::plan::PlanConstraints::may_trace_duplicate_edges`])*

// FIXME: The entire vo_bit module should only be available if the "vo_bit" feature is enabled.
// However, the malloc-based MarkSweepSpace and Lisp2Space depends on the VO bits regardless
// of the "vo_bit" feature.
#[cfg(feature = "vo_bit")]
pub(crate) mod helper;

use atomic::Ordering;

use crate::util::metadata::side_metadata::SideMetadataSpec;
use crate::util::Address;
use crate::util::ObjectReference;
use crate::vm::object_model::ObjectModel;
use crate::vm::VMBinding;

cfg_if::cfg_if! {
    if #[cfg(feature = "vo_bit_access")] {
        /// A VO bit is required per min-object-size aligned address, rather than per object, and can only exist as side metadata.
        /// This is only publicly available when the feature "vo_bit_access" is enabled.
        /// Check the comments on "vo_bit_access" in `Cargo.toml` before use. Use at your own risk.
        pub const VO_BIT_SIDE_METADATA_SPEC: SideMetadataSpec =
        crate::util::metadata::side_metadata::spec_defs::VO_BIT;
    } else {
        /// A VO bit is required per min-object-size aligned address, rather than per object, and can only exist as side metadata.
        pub(crate) const VO_BIT_SIDE_METADATA_SPEC: SideMetadataSpec =
            crate::util::metadata::side_metadata::spec_defs::VO_BIT;
    }
}

/// The base address for VO bit side metadata on 64 bits platforms.
#[cfg(target_pointer_width = "64")]
pub fn vo_bit_side_metadata_addr() -> Address {
    VO_BIT_SIDE_METADATA_SPEC.get_starting_address()
}

/// Atomically set the VO bit for an object.
pub(crate) fn set_vo_bit(object: ObjectReference) {
    debug_assert!(!is_vo_bit_set(object), "{:x}: VO bit already set", object);
    VO_BIT_SIDE_METADATA_SPEC.store_atomic::<u8>(object.to_raw_address(), 1, Ordering::SeqCst);
}

/// Atomically unset the VO bit for an object.
pub(crate) fn unset_vo_bit(object: ObjectReference) {
    debug_assert!(is_vo_bit_set(object), "{:x}: VO bit not set", object);
    VO_BIT_SIDE_METADATA_SPEC.store_atomic::<u8>(object.to_raw_address(), 0, Ordering::SeqCst);
}

/// Atomically unset the VO bit for an object, regardless whether the bit is set or not.
pub(crate) fn unset_vo_bit_nocheck(object: ObjectReference) {
    VO_BIT_SIDE_METADATA_SPEC.store_atomic::<u8>(object.to_raw_address(), 0, Ordering::SeqCst);
}

/// Check if the VO bit is set for an object.
pub(crate) fn is_vo_bit_set(object: ObjectReference) -> bool {
    VO_BIT_SIDE_METADATA_SPEC.load_atomic::<u8>(object.to_raw_address(), Ordering::SeqCst) == 1
}

/// Check if an address can be turned directly into an object reference using the VO bit.
/// If so, return `Some(object)`. Otherwise return `None`.
///
/// The `address` must be word-aligned.
pub(crate) fn is_vo_bit_set_for_addr(address: Address) -> Option<ObjectReference> {
    is_vo_bit_set_for_addr_order(address, Ordering::SeqCst)
}

/// Check if an address can be turned directly into an object reference using an atomic VO-bit load
/// with the given memory ordering.
///
/// The `address` must be word-aligned.
pub(crate) fn is_vo_bit_set_for_addr_order(
    address: Address,
    order: Ordering,
) -> Option<ObjectReference> {
    check_vo_bit(address, |addr| {
        VO_BIT_SIDE_METADATA_SPEC.load_atomic::<u8>(addr, order)
    })
}

fn check_vo_bit(addr: Address, load_vo_bit: impl FnOnce(Address) -> u8) -> Option<ObjectReference> {
    debug_assert!(
        addr.is_aligned_to(ObjectReference::ALIGNMENT),
        "Address is not word-aligned: {addr}"
    );

    // If we haven't mapped VO bit for the address, it cannot be an object
    if !VO_BIT_SIDE_METADATA_SPEC.is_mapped(addr) {
        return None;
    }

    if load_vo_bit(addr) == 1 {
        ObjectReference::from_raw_address(addr)
    } else {
        None
    }
}

/// Project a non-empty sub-range `[grain_start, grain_end)` within a single mapped `MMAPPER`
/// grain of `spec`'s reserved side-metadata window as `&[AtomicU8]` for the duration of `f`
/// (dropping the slice before returning), or return `None` if side metadata is uninitialized or
/// `grain_start` is unmapped.
///
/// Module-private to `vo_bit`: invoked only for `VO_BIT_SIDE_METADATA_SPEC` (accessed exclusively
/// via `AtomicU8` crate-wide) and quiescent `LOCAL_MARK_BIT_SPEC` during region sweeping
/// (`bcopy_vo_bit_from_side_spec`).
#[inline]
fn with_mapped_side_meta_grain<R>(
    spec: &SideMetadataSpec,
    chunk_anchor: Address,
    grain_start: Address,
    grain_end: Address,
    f: impl FnOnce(&[core::sync::atomic::AtomicU8]) -> R,
) -> Option<R> {
    use crate::util::metadata::side_metadata::{helpers, try_side_metadata_reserved_range};
    use core::sync::atomic::AtomicU8;

    if grain_start >= grain_end || grain_start.is_zero() {
        return None;
    }
    let (reserved_start, reserved_end) = try_side_metadata_reserved_range()?;
    helpers::assert_meta_range_within_reserved(
        spec,
        chunk_anchor,
        reserved_start,
        reserved_end,
        grain_start,
        grain_end,
    );
    let grain = crate::MMAPPER.granularity();
    assert_eq!(
        grain_start.align_down(grain),
        (grain_end - 1usize).align_down(grain),
        "Side-metadata range [{grain_start}, {grain_end}) crosses MMAPPER grain boundary"
    );
    if !grain_start.is_mapped() {
        return None;
    }
    let len = grain_end - grain_start;
    assert!(len <= isize::MAX as usize);
    // SAFETY: `try_side_metadata_reserved_range` + `assert_meta_range_within_reserved` verified
    // initialization and that `[grain_start, grain_end)` lies strictly within MMTk's reserved
    // side-metadata window and `spec`'s bounds; `grain_start` sits in a single mapped,
    // OS-zero-initialized `MMAPPER` grain (`AtomicU8` align 1, `len <= isize::MAX`), `slice` does
    // not escape `f`, and all module callers pass either globally `AtomicU8`-only
    // `VO_BIT_SIDE_METADATA_SPEC` or quiescent `LOCAL_MARK_BIT_SPEC` during region sweeping without
    // non-atomic write races or `&mut` aliases.
    let slice = unsafe { core::slice::from_raw_parts(grain_start.to_ptr::<AtomicU8>(), len) };
    Some(f(slice))
}

#[inline]
fn apply_masked_vo_slice(
    dst_slice: &[core::sync::atomic::AtomicU8],
    first_mask: u8,
    last_mask: u8,
    mut masked: impl FnMut(usize, &core::sync::atomic::AtomicU8, u8),
    mut full: impl FnMut(usize, &core::sync::atomic::AtomicU8),
) {
    let len = dst_slice.len();
    if len == 1 {
        let mask = first_mask & last_mask;
        if mask == 0xff {
            full(0, &dst_slice[0]);
        } else {
            masked(0, &dst_slice[0], mask);
        }
        return;
    }
    let mid_start = if first_mask != 0xff {
        masked(0, &dst_slice[0], first_mask);
        1
    } else {
        0
    };
    let mid_end = if last_mask != 0xff {
        masked(len - 1, &dst_slice[len - 1], last_mask);
        len - 1
    } else {
        len
    };
    for (idx, atom) in dst_slice.iter().enumerate().take(mid_end).skip(mid_start) {
        full(idx, atom);
    }
}

/// Bulk zero the VO bit atomically (`AtomicU8` stores on full interior bytes, `fetch_and` on
/// partial boundary bytes) so concurrent atomic `VO_BIT` readers (`enumerate_objects`,
/// `is_mmtk_object`, `find_object_from_internal_pointer`, `MallocSpace::vo_bit_bytes`) never
/// encounter a non-atomic `memset` data race or `&mut [u8]` alias.
pub(crate) fn bzero_vo_bit(start: Address, size: usize) {
    use crate::util::metadata::side_metadata::helpers;
    use core::ops::ControlFlow;

    #[cfg(feature = "extreme_assertions")]
    let _lock = crate::util::metadata::side_metadata::sanity::SANITY_LOCK
        .lock()
        .unwrap();
    #[cfg(feature = "extreme_assertions")]
    crate::util::metadata::side_metadata::sanity::verify_bzero(
        &VO_BIT_SIDE_METADATA_SPEC,
        start,
        size,
    );

    let Some((byte_start, byte_end, first_mask, last_mask)) =
        helpers::half_open_contiguous_meta_range(&VO_BIT_SIDE_METADATA_SPEC, start, size)
    else {
        return;
    };
    helpers::try_for_each_meta_grain_forward(
        byte_start,
        byte_end,
        first_mask,
        last_mask,
        |g_start, g_end, g_first, g_last| {
            with_mapped_side_meta_grain(
                &VO_BIT_SIDE_METADATA_SPEC,
                start,
                g_start,
                g_end,
                |slice| {
                    apply_masked_vo_slice(
                        slice,
                        g_first,
                        g_last,
                        |_, atom, mask| {
                            atom.fetch_and(!mask, Ordering::SeqCst);
                        },
                        |_, atom| atom.store(0, Ordering::Relaxed),
                    );
                },
            )
            .expect("bzero_vo_bit called on unmapped VO-bit side metadata");
            ControlFlow::<()>::Continue(())
        },
    );
}

/// Bulk copy VO bits from side mark bits atomically (`AtomicU8` loads/stores on interior bytes,
/// `fetch_update` on partial boundary bytes). Called exclusively from
/// `vo_bit::helper::on_region_swept` when sweeping an occupied region after marking completes.
#[cfg(feature = "vo_bit")]
fn bcopy_vo_bit_from_mark_bit<VM: VMBinding>(start: Address, size: usize) {
    let mark_bit_spec = VM::VMObjectModel::LOCAL_MARK_BIT_SPEC;
    assert!(
        mark_bit_spec.is_on_side(),
        "bcopy_vo_bit_from_mark_bits can only be used with on-the-side mark bits."
    );
    bcopy_vo_bit_from_side_spec(start, size, mark_bit_spec.extract_side_spec());
}

#[cfg(any(feature = "vo_bit", test))]
fn bcopy_vo_bit_from_side_spec(start: Address, size: usize, side_mark_bit_spec: &SideMetadataSpec) {
    use crate::util::metadata::side_metadata::helpers;

    #[cfg(feature = "extreme_assertions")]
    let _lock = crate::util::metadata::side_metadata::sanity::SANITY_LOCK
        .lock()
        .unwrap();
    #[cfg(feature = "extreme_assertions")]
    crate::util::metadata::side_metadata::sanity::verify_bcopy(
        &VO_BIT_SIDE_METADATA_SPEC,
        start,
        size,
        side_mark_bit_spec,
    );

    assert!(
        !side_mark_bit_spec.is_global
            && side_mark_bit_spec.log_bytes_in_region
                == VO_BIT_SIDE_METADATA_SPEC.log_bytes_in_region
            && side_mark_bit_spec.log_num_of_bits == VO_BIT_SIDE_METADATA_SPEC.log_num_of_bits,
        "bcopy_vo_bit_from_mark_bit expects matching local side mark-bit spec: {side_mark_bit_spec:?}"
    );

    helpers::for_each_spec_data_span(side_mark_bit_spec, start, size, |sub_start, sub_size| {
        let Some((dst_byte_start, dst_byte_end, first_mask, last_mask)) =
            helpers::half_open_contiguous_meta_range(
                &VO_BIT_SIDE_METADATA_SPEC,
                sub_start,
                sub_size,
            )
        else {
            return;
        };
        let src_byte_start = helpers::address_to_meta_address(side_mark_bit_spec, sub_start);
        debug_assert_eq!(
            helpers::meta_byte_lshift(&VO_BIT_SIDE_METADATA_SPEC, sub_start),
            helpers::meta_byte_lshift(side_mark_bit_spec, sub_start)
        );
        let total_bytes = dst_byte_end - dst_byte_start;
        let grain = crate::MMAPPER.granularity();
        let mut offset = 0usize;
        while offset < total_bytes {
            let dst_cur = dst_byte_start + offset;
            let src_cur = src_byte_start + offset;
            let step = (total_bytes - offset)
                .min(helpers::grain_rem(dst_cur, grain))
                .min(helpers::grain_rem(src_cur, grain));
            let g_first = if offset == 0 { first_mask } else { 0xff };
            let g_last = if offset + step == total_bytes {
                last_mask
            } else {
                0xff
            };
            with_mapped_side_meta_grain(
                side_mark_bit_spec,
                sub_start,
                src_cur,
                src_cur + step,
                |src_slice| {
                    with_mapped_side_meta_grain(
                        &VO_BIT_SIDE_METADATA_SPEC,
                        sub_start,
                        dst_cur,
                        dst_cur + step,
                        |dst_slice| {
                            apply_masked_vo_slice(
                                dst_slice,
                                g_first,
                                g_last,
                                |i, dst_atom, mask| {
                                    let src_val = src_slice[i].load(Ordering::Relaxed);
                                    let _ = dst_atom.fetch_update(
                                        Ordering::Release,
                                        Ordering::Relaxed,
                                        |old| Some((src_val & mask) | (old & !mask)),
                                    );
                                },
                                |i, dst_atom| {
                                    dst_atom.store(
                                        src_slice[i].load(Ordering::Relaxed),
                                        Ordering::Release,
                                    );
                                },
                            );
                        },
                    )
                    .expect("bcopy_vo_bit_from_mark_bit called on unmapped VO-bit metadata");
                },
            )
            .expect("bcopy_vo_bit_from_mark_bit called on unmapped mark-bit metadata");
            offset += step;
        }
    });
}

use crate::util::constants::{LOG_BITS_IN_BYTE, LOG_BYTES_IN_ADDRESS};

/// How many data memory bytes does 1 word in the VO bit side metadata represents?
pub(crate) const VO_BIT_WORD_TO_REGION: usize = 1
    << (VO_BIT_SIDE_METADATA_SPEC.log_bytes_in_region
        + LOG_BITS_IN_BYTE as usize
        + LOG_BYTES_IN_ADDRESS as usize
        - VO_BIT_SIDE_METADATA_SPEC.log_num_of_bits);

/// Atomically scan all set VO bits in the half-open data address range `[start, end)`, invoking
/// `callback` in ascending address order for each valid `ObjectReference`.
///
/// Unmapped side-metadata grains within `[start, end)` are skipped cleanly, and mapped grains are
/// read in a single pass per grain via `AtomicU8` (`Ordering::Relaxed` 8-byte zero skip +
/// `Ordering::Acquire` per-byte loads) so heap enumeration (`MMTK::enumerate_objects`) never causes
/// a Rust data race if mutator or GC threads concurrently update VO bits elsewhere.
pub(crate) fn scan_vo_bits_atomic(
    start: Address,
    end: Address,
    mut callback: impl FnMut(ObjectReference),
) {
    use crate::util::metadata::side_metadata::{helpers, is_side_metadata_initialized};
    use core::ops::ControlFlow;

    const REGION_BYTES: usize = 1 << VO_BIT_SIDE_METADATA_SPEC.log_bytes_in_region;
    const MAX_ALIGNED: usize = helpers::CONTIGUOUS_DATA_MAX & !(REGION_BYTES - 1);

    if start >= end || start.as_usize() > MAX_ALIGNED || !is_side_metadata_initialized() {
        return;
    }
    let last_raw = (end - 1usize).as_usize().min(helpers::CONTIGUOUS_DATA_MAX);
    let first_data = start.align_up(REGION_BYTES);
    let last_data = (start + (last_raw - start.as_usize())).align_down(REGION_BYTES);
    let Some((byte_start, byte_end, first_mask, last_mask)) =
        helpers::inclusive_contiguous_meta_range(&VO_BIT_SIDE_METADATA_SPEC, first_data, last_data)
    else {
        return;
    };

    helpers::try_for_each_meta_grain_forward(
        byte_start,
        byte_end,
        first_mask,
        last_mask,
        |g_start, g_end, g_first, g_last| {
            let _ = with_mapped_side_meta_grain(
                &VO_BIT_SIDE_METADATA_SPEC,
                first_data,
                g_start,
                g_end,
                |slice| {
                    helpers::scan_non_zero_atomic_bits_in_slice(
                        slice,
                        g_first,
                        g_last,
                        |byte_off, bit| {
                            let vo_addr = helpers::contiguous_meta_address_to_address(
                                &VO_BIT_SIDE_METADATA_SPEC,
                                g_start + byte_off,
                                bit,
                            );
                            debug_assert!(vo_addr >= first_data && vo_addr <= last_data);
                            if let Some(obj) = ObjectReference::from_raw_address(vo_addr) {
                                callback(obj);
                            }
                        },
                    );
                },
            );
            ControlFlow::<()>::Continue(())
        },
    );
}

/// Atomically search backwards from `start` over at most `search_limit_bytes` bytes (`[start -
/// search_limit_bytes + 1, start]`) for the highest region-aligned address whose VO bit is set,
/// stopping at the first unmapped metadata grain going backwards.
fn find_prev_vo_bit_atomic(start: Address, search_limit_bytes: usize) -> Option<Address> {
    use crate::util::metadata::side_metadata::{helpers, is_side_metadata_initialized};
    use core::ops::ControlFlow;

    const REGION_BYTES: usize = 1 << VO_BIT_SIDE_METADATA_SPEC.log_bytes_in_region;
    const MAX_ALIGNED: usize = helpers::CONTIGUOUS_DATA_MAX & !(REGION_BYTES - 1);

    if search_limit_bytes == 0
        || start.as_usize() > helpers::CONTIGUOUS_DATA_MAX
        || !is_side_metadata_initialized()
        || !VO_BIT_SIDE_METADATA_SPEC.is_mapped(start)
    {
        return None;
    }

    let lower_bound = start.saturating_sub(search_limit_bytes - 1);
    if lower_bound.as_usize() > MAX_ALIGNED {
        return None;
    }

    let first_data = lower_bound.align_up(REGION_BYTES);
    let last_data = start.align_down(REGION_BYTES);
    let (byte_start, byte_end, first_mask, last_mask) = helpers::inclusive_contiguous_meta_range(
        &VO_BIT_SIDE_METADATA_SPEC,
        first_data,
        last_data,
    )?;

    helpers::try_for_each_meta_grain_backward(
        byte_start,
        byte_end,
        first_mask,
        last_mask,
        |g_start, g_end, g_first, g_last| {
            let Some(found) = with_mapped_side_meta_grain(
                &VO_BIT_SIDE_METADATA_SPEC,
                first_data,
                g_start,
                g_end,
                |slice| helpers::find_last_non_zero_atomic_byte(slice, g_first, g_last),
            ) else {
                return ControlFlow::Break(None);
            };
            if let Some((byte_offset, bits)) = found {
                debug_assert_ne!(bits, 0);
                let highest_bit = 7 - bits.leading_zeros() as u8;
                let vo_addr = helpers::contiguous_meta_address_to_address(
                    &VO_BIT_SIDE_METADATA_SPEC,
                    g_start + byte_offset,
                    highest_bit,
                );
                debug_assert!(vo_addr >= first_data && vo_addr <= last_data);
                return ControlFlow::Break(Some(vo_addr));
            }
            ControlFlow::Continue(())
        },
    )
    .flatten()
}

/// Find the base reference to the object from a potential internal pointer.
pub(crate) fn find_object_from_internal_pointer<VM: VMBinding>(
    start: Address,
    search_limit_bytes: usize,
) -> Option<ObjectReference> {
    let vo_addr = find_prev_vo_bit_atomic(start, search_limit_bytes)?;
    is_internal_ptr_from_vo_bit::<VM>(vo_addr, start)
}

/// Get the object reference from an aligned address where VO bit is set.
pub(crate) fn get_object_ref_for_vo_addr(vo_addr: Address) -> ObjectReference {
    // VO bit should be set on the address.
    debug_assert!(vo_addr.is_aligned_to(ObjectReference::ALIGNMENT));
    debug_assert!(
        VO_BIT_SIDE_METADATA_SPEC.is_mapped(vo_addr)
            && VO_BIT_SIDE_METADATA_SPEC.load_atomic::<u8>(vo_addr, Ordering::Relaxed) != 0
    );
    ObjectReference::from_raw_address(vo_addr).expect("VO bit cannot be set at null address")
}

/// Check if the address could be an internal pointer in the object.
fn is_internal_ptr<VM: VMBinding>(obj: ObjectReference, internal_ptr: Address) -> bool {
    let obj_start = obj.to_object_start::<VM>();
    let obj_size = VM::VMObjectModel::get_current_size(obj);
    internal_ptr < obj_start + obj_size
}

/// Check if the address could be an internal pointer based on where VO bit is set.
pub(crate) fn is_internal_ptr_from_vo_bit<VM: VMBinding>(
    vo_addr: Address,
    internal_ptr: Address,
) -> Option<ObjectReference> {
    let obj = get_object_ref_for_vo_addr(vo_addr);
    if is_internal_ptr::<VM>(obj, internal_ptr) {
        Some(obj)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::constants::BYTES_IN_PAGE;
    use crate::util::heap::layout::vm_layout;
    use crate::util::metadata::side_metadata::spec_defs::MS_OFFSET_MALLOC;
    use crate::util::metadata::side_metadata::{
        core_test_initialize_side_metadata, sanity, SideMetadataContext, SideMetadataSanity,
    };
    use crate::util::test_util::{serial_test, with_cleanup};

    #[test]
    fn test_vo_bit_atomic_scan_prev_bzero_and_bcopy() {
        serial_test(|| {
            core_test_initialize_side_metadata();
            let base = vm_layout::vm_layout().heap_start;
            let region = 1usize << VO_BIT_SIDE_METADATA_SPEC.log_bytes_in_region;
            let data_per_meta_grain = crate::MMAPPER.granularity() * 8 * region;
            // Map grains 0 (`base`), 2 (`grain2`), and 3 (`grain3`), leaving grain 1 (`grain1`)
            // unmapped to exercise both forward gap skipping and backward unmapped-grain stopping.
            let grain1 = base + data_per_meta_grain;
            let grain2 = base + data_per_meta_grain * 2;
            let grain3 = base + data_per_meta_grain * 3;
            let test_mark_spec = SideMetadataSpec {
                name: "vo_test_local_mark",
                is_global: false,
                offset: MS_OFFSET_MALLOC.offset,
                log_num_of_bits: VO_BIT_SIDE_METADATA_SPEC.log_num_of_bits,
                log_bytes_in_region: VO_BIT_SIDE_METADATA_SPEC.log_bytes_in_region,
            };
            let context = SideMetadataContext {
                global: vec![VO_BIT_SIDE_METADATA_SPEC],
                local: vec![test_mark_spec],
            };
            let mut meta_sanity = SideMetadataSanity::new();
            meta_sanity.verify_metadata_context("VoAtomicSuite", &context);
            let obj_at = |addr: Address| ObjectReference::from_raw_address(addr).unwrap();

            with_cleanup(
                || {
                    for (g, tag) in [(base, "vo_g0"), (grain2, "vo_g2"), (grain3, "vo_g3")] {
                        context
                            .try_map_metadata_space(g, BYTES_IN_PAGE * 4, tag)
                            .unwrap();
                        bzero_vo_bit(g, BYTES_IN_PAGE * 4);
                        test_mark_spec.bzero_metadata(g, BYTES_IN_PAGE * 4);
                    }

                    // 1. Forward atomic scan + unmapped gap + re-entrant `bzero_vo_bit`.
                    let (o0, o1, o2, o3) = (
                        obj_at(base),
                        obj_at(base + region * 3),
                        obj_at(base + region * 9),
                        obj_at(grain2 + region * 5),
                    );
                    for o in [o0, o1, o2, o3] {
                        set_vo_bit(o);
                    }
                    let collect = |s: Address, e: Address| {
                        let mut v = Vec::new();
                        scan_vo_bits_atomic(s, e, |o| v.push(o));
                        v
                    };
                    assert!(collect(base, base).is_empty());
                    assert!(collect(base + 1usize, base + region - 1).is_empty());
                    assert!(collect(Address::MAX - 64usize, Address::MAX).is_empty());
                    assert_eq!(collect(base + 1usize, base + region * 9), vec![o1]);
                    assert_eq!(collect(base, grain2 + BYTES_IN_PAGE), vec![o0, o1, o2, o3]);

                    // 2. Backward `find_prev_vo_bit_atomic` boundaries + multi-grain + unmapped gap.
                    assert_eq!(find_prev_vo_bit_atomic(o0.to_raw_address(), 0), None);
                    assert_eq!(find_prev_vo_bit_atomic(Address::MAX, usize::MAX), None);
                    assert_eq!(find_prev_vo_bit_atomic(grain1 + 64usize, usize::MAX), None);
                    let sz = region * 4;
                    assert_eq!(
                        find_prev_vo_bit_atomic(o2.to_raw_address() + (sz - 1), sz),
                        Some(o2.to_raw_address())
                    );
                    assert_eq!(find_prev_vo_bit_atomic(o2.to_raw_address() + sz, sz), None);
                    assert_eq!(
                        find_prev_vo_bit_atomic(o2.to_raw_address() + region * 2, usize::MAX),
                        Some(o2.to_raw_address())
                    );
                    assert_eq!(
                        find_prev_vo_bit_atomic(grain3 + BYTES_IN_PAGE, data_per_meta_grain * 2),
                        Some(o3.to_raw_address())
                    );

                    // Re-entrant callback clears `base`, leaving `o3`; unsetting `o3` then causes
                    // backward search from `grain2` to stop at unmapped `grain1`.
                    let mut seen = Vec::new();
                    scan_vo_bits_atomic(base, grain2 + BYTES_IN_PAGE, |o| {
                        seen.push(o);
                        if o == o0 {
                            bzero_vo_bit(base, BYTES_IN_PAGE * 4);
                        }
                    });
                    assert_eq!(seen, vec![o0, o1, o3]);
                    unset_vo_bit(o3);
                    assert_eq!(
                        find_prev_vo_bit_atomic(grain2 + BYTES_IN_PAGE, usize::MAX),
                        None
                    );

                    // 3. Partial + multi-byte atomic `bzero_vo_bit` and `bcopy_vo_bit_from_side_spec`.
                    bzero_vo_bit(base, 0);
                    bcopy_vo_bit_from_side_spec(base, 0, &test_mark_spec);
                    for i in [0usize, 1, 3, 5, 7, 8, 19, 80, 95, 96] {
                        set_vo_bit(obj_at(base + region * i));
                    }
                    bzero_vo_bit(base + region * 2, region * 4);
                    bzero_vo_bit(base + region * 7, region * 89);
                    assert_eq!(
                        collect(base, base + region * 100),
                        vec![
                            obj_at(base),
                            obj_at(base + region),
                            obj_at(base + region * 96)
                        ]
                    );

                    for i in [1usize, 4, 17, 73, 94] {
                        test_mark_spec.store_atomic::<u8>(base + region * i, 1, Ordering::SeqCst);
                    }
                    bcopy_vo_bit_from_side_spec(base + region * 3, region * 92, &test_mark_spec);
                    let expected: Vec<_> = [0usize, 1, 4, 17, 73, 94, 96]
                        .into_iter()
                        .map(|i| obj_at(base + region * i))
                        .collect();
                    assert_eq!(collect(base, base + region * 100), expected);
                },
                || {
                    for g in [base, grain2, grain3] {
                        if VO_BIT_SIDE_METADATA_SPEC.is_mapped(g) {
                            bzero_vo_bit(g, BYTES_IN_PAGE * 4);
                        }
                        if test_mark_spec.is_mapped(g) {
                            test_mark_spec.bzero_metadata(g, BYTES_IN_PAGE * 4);
                        }
                    }
                    sanity::reset();
                },
            );
        });
    }
}
