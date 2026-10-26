use std::marker::PhantomData;
use std::sync::atomic::AtomicUsize;

use crate::util::linear_scan::Region;
use crate::util::Address;
use crate::{
    policy::immix::line::Line,
    util::{metadata::side_metadata::SideMetadataSpec, ObjectReference},
    vm::*,
};
use atomic::Ordering;

/// Log2 of the number of bits used to store each object's reference count in the RC table.
pub const LOG_REF_COUNT_BITS: usize = 1;
/// Number of bits used to store each object's reference count in the RC table.
pub const REF_COUNT_BITS: u8 = 1 << LOG_REF_COUNT_BITS;
/// Bit mask covering the bits used to store a reference count.
pub const REF_COUNT_MASK: u8 = (((1u16 << REF_COUNT_BITS) - 1) & 0xff) as u8;
/// The maximum representable reference count. Once an object's count reaches this value it
/// is treated as saturated/sticky and is no longer incremented or decremented.
pub const MAX_REF_COUNT: u8 = REF_COUNT_MASK;

/// Log2 of the minimum object size, i.e. the granularity at which reference counts are tracked.
pub const LOG_MIN_OBJECT_SIZE: usize = crate::util::constants::LOG_MIN_OBJECT_SIZE as _;
/// The minimum object size, i.e. the granularity at which reference counts are tracked.
pub const MIN_OBJECT_SIZE: usize = 1 << LOG_MIN_OBJECT_SIZE;

/// Side metadata recording which Immix lines are "straddled" by an object that spans
/// multiple lines, so straddling objects can be identified without scanning their contents.
pub const RC_STRADDLE_LINES: SideMetadataSpec =
    crate::util::metadata::side_metadata::spec_defs::RC_STRADDLE_LINES;

/// Side metadata spec for the per-object reference count table.
pub const RC_TABLE: SideMetadataSpec = crate::util::metadata::side_metadata::spec_defs::RC_TABLE;

static INC_BUFFER_SIZE: AtomicUsize = AtomicUsize::new(0);

/// A zero-sized helper type providing methods to read and update per-object reference count
/// metadata for LXR's reference counting plan.
#[repr(transparent)]
#[derive(Debug, Copy)]
pub struct RefCountHelper<VM: VMBinding>(PhantomData<VM>);

impl<VM: VMBinding> RefCountHelper<VM> {
    /// A singleton instance of `RefCountHelper` (the type is zero-sized, so it can be freely copied/cloned).
    pub const NEW: Self = Self(PhantomData);
    /// Whether extra reference-counting sanity checks are enabled (debug builds or the `sanity` feature).
    pub const SANITY: bool = cfg!(debug_assertions) || cfg!(feature = "sanity");

    /// Returns the current size of the global increment buffer, i.e. the number of pending
    /// reference count increments that have been enqueued but not yet processed.
    pub fn inc_buffer_size(&self) -> usize {
        INC_BUFFER_SIZE.load(Ordering::Relaxed)
    }

    /// Increases the global increment buffer size counter by `delta`.
    pub fn increase_inc_buffer_size(&self, delta: usize) {
        // `update()` allows infallable atomic update, but is not stablized until 1.95.
        // Migrate to `update()` after bumping MSRV.
        #[allow(deprecated)]
        let _ = INC_BUFFER_SIZE.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |x| {
            Some(x.saturating_add(delta))
        });
    }

    /// Resets the global increment buffer size counter to zero.
    pub fn reset_inc_buffer_size(&self) {
        INC_BUFFER_SIZE.store(0, Ordering::Relaxed)
    }

    /// Atomically updates the reference count of object `o` by applying `f` to its current
    /// value, following the same semantics as `AtomicU8::fetch_update`.
    #[inline(always)]
    pub fn fetch_update(
        &self,
        o: ObjectReference,
        f: impl FnMut(u8) -> Option<u8>,
    ) -> Result<u8, u8> {
        RC_TABLE.fetch_update_atomic(o.to_raw_address(), Ordering::Relaxed, Ordering::Relaxed, f)
    }

    /// Increments object `o`'s reference count by one, leaving it unchanged (saturating) once
    /// it has reached `MAX_REF_COUNT`.
    #[inline(always)]
    pub fn inc(&self, o: ObjectReference) -> Result<u8, u8> {
        #[cfg(feature = "vo_bit")]
        debug_assert!(
            crate::util::metadata::vo_bit::is_vo_bit_set(o),
            "{o}: VO bit not set",
        );

        self.fetch_update(o, |x| {
            debug_assert!(x <= MAX_REF_COUNT);
            if x == MAX_REF_COUNT {
                None
            } else {
                Some(x + 1)
            }
        })
    }

    /// Atomically sets object `o`'s reference count to `count`.
    #[inline(always)]
    pub fn set(&self, o: ObjectReference, count: u8) {
        RC_TABLE.store_atomic(o.to_raw_address(), count, Ordering::Relaxed)
    }

    /// Returns object `o`'s current reference count.
    #[inline(always)]
    pub fn count(&self, o: ObjectReference) -> u8 {
        RC_TABLE.load_atomic(o.to_raw_address(), Ordering::Relaxed)
    }

    /// Returns the reference count stored in the RC table at address `addr`. If this
    /// returns a non-zero value, it indicates that `addr` is the address of an object reference,
    /// or the start of a straddle line.
    #[inline(always)]
    pub fn count_by_address(&self, addr: Address) -> u8 {
        RC_TABLE.load_atomic(addr, Ordering::Relaxed)
    }

    /// Returns `true` if object `o`'s reference count is zero.
    #[allow(unused)]
    #[inline(always)]
    pub fn is_dead(&self, o: ObjectReference) -> bool {
        let v: u8 = RC_TABLE.load_atomic(o.to_raw_address(), Ordering::Relaxed);
        v == 0
    }

    /// Returns `true` if object `o`'s reference count is zero (dead) or has saturated at
    /// `MAX_REF_COUNT` (sticky).
    #[inline(always)]
    pub fn is_dead_or_stuck(&self, o: ObjectReference) -> bool {
        let v: u8 = RC_TABLE.load_atomic(o.to_raw_address(), Ordering::Relaxed);
        v == 0 || v == MAX_REF_COUNT
    }

    /// Returns `true` if object `o` is in a straddle line. The function does not check rc table.
    #[inline(always)]
    pub fn object_is_in_straddle_line_no_rc_check(&self, o: ObjectReference) -> bool {
        // Directly reads line-granularity straddle line metadata atomically; `SideMetadataSpec`
        // shifts `o.to_raw_address()` by `Line::LOG_BYTES` to index the containing line's byte.
        RC_STRADDLE_LINES.load_atomic::<u8>(o.to_raw_address(), Ordering::Relaxed) != 0
    }

    /// Returns `true` if address `a` falls within a live object whose containing line is marked
    /// as a straddle line.
    #[inline(always)]
    pub fn object_is_in_straddle_line(&self, o: ObjectReference) -> bool {
        let line = Line::from_unaligned_address(o.to_raw_address());
        self.count(o) != 0
            && RC_STRADDLE_LINES.load_atomic::<u8>(line.start(), Ordering::Relaxed) != 0
    }

    fn mark_straddle_object_with_size(&self, o: ObjectReference, size: usize) {
        debug_assert!(size > Line::BYTES);
        let start = o.to_object_start::<VM>();
        let end = start + size;
        // Skip the line holding the object's reference address. As we currently conservatively skip
        // the last line in a hole and do not use that line, we don't need to mark lines at object start here
        // (as the line would be convervatively kept alive). See: https://github.com/mmtk/mmtk-core/pull/1576
        // TODO: if we use a different solution to handle object start, we may need to mark lines
        // at object start here.
        let start_line = Line::from_unaligned_address(o.to_raw_address()).next();
        let end_line = Line::from_unaligned_address(end);
        // Note that `end_line` may be the last line overlapping with `o`.
        // In that case, `end_line` will not be marked.
        // It is OK because when searching for available lines (`rc_get_next_available_lines`),
        // it always skips the first line in a hole.
        let mut line = start_line;
        while line < end_line {
            RC_STRADDLE_LINES.store_atomic(line.start(), 1u8, Ordering::Relaxed);
            RC_TABLE.store_atomic(line.start(), 1u8, Ordering::Relaxed);
            line = line.next();
        }
    }

    /// Marks every line (other than the first) spanned by object `o` as a straddle line, so the
    /// object can be identified from any of the lines it straddles.
    pub fn mark_straddle_object(&self, o: ObjectReference) {
        let size = VM::VMObjectModel::get_current_size(o);
        self.mark_straddle_object_with_size(o, size)
    }

    /// Clears the straddle-line and reference-count markers set by `mark_straddle_object` for
    /// every line (other than the first) spanned by object `o`.
    pub fn unmark_straddle_object(&self, o: ObjectReference) {
        let size = VM::VMObjectModel::get_current_size(o);
        if size > Line::BYTES {
            let start = o.to_object_start::<VM>();
            let end = start + size;
            // Must match `mark_straddle_object_with_size` exactly; see the comments there.
            let start_line = Line::from_unaligned_address(o.to_raw_address()).next();
            let end_line = Line::from_unaligned_address(end);
            // Note that `end_line` may be the last line overlapping with `o`.
            // In that case, `end_line` will not be marked.
            // It is OK because when searching for available lines (`rc_get_next_available_lines`),
            // it always skips the first line in a hole.
            let mut line = start_line;
            while line < end_line {
                RC_TABLE.store_atomic(line.start(), 0u8, Ordering::Relaxed);
                RC_STRADDLE_LINES.store_atomic(line.start(), 0u8, Ordering::Relaxed);
                line = line.next();
            }
        }
    }

    /// Marks object `o` as a straddle object when promoted to mature space if its supplied `size`
    /// spans more than one line.
    #[inline(always)]
    pub fn promote_with_size(&self, o: ObjectReference, size: usize) {
        if size > Line::BYTES {
            self.mark_straddle_object_with_size(o, size);
        }
    }
}

impl<VM: VMBinding> Clone for RefCountHelper<VM> {
    fn clone(&self) -> Self {
        Self(PhantomData)
    }
}
