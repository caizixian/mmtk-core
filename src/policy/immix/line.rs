use std::ops::Range;
use std::sync::atomic::AtomicU8;

use super::block::Block;
use crate::util::constants::LOG_BITS_IN_BYTE;
use crate::util::linear_scan::{Region, RegionIterator};
use crate::util::metadata::side_metadata::spec_defs::IX_LINE_REUSE_COUNT;
use crate::util::metadata::side_metadata::*;
use crate::util::rc;
use crate::{
    util::{Address, ObjectReference},
    vm::*,
};
use atomic::Ordering;

/// Data structure to reference a line within an immix block.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialOrd, PartialEq, Eq)]
pub struct Line(Address);

impl Region for Line {
    const LOG_BYTES: usize = 8;

    #[allow(clippy::assertions_on_constants)] // make sure line is not used when BLOCK_ONLY is turned on.
    fn from_aligned_address(address: Address) -> Self {
        debug_assert!(!super::BLOCK_ONLY);
        debug_assert!(address.is_aligned_to(Self::BYTES));
        Self(address)
    }

    fn start(&self) -> Address {
        self.0
    }
}

#[allow(clippy::assertions_on_constants)]
impl Line {
    pub const RESET_MARK_STATE: u8 = 1;
    pub const MAX_MARK_STATE: u8 = 127;

    /// Line mark table (side)
    pub const MARK_TABLE: SideMetadataSpec =
        crate::util::metadata::side_metadata::spec_defs::IX_LINE_MARK;

    /// Get the block containing the line.
    pub fn block(&self) -> Block {
        debug_assert!(!super::BLOCK_ONLY);
        Block::from_unaligned_address(self.0)
    }

    /// Get line index within its containing block.
    pub fn get_index_within_block(&self) -> usize {
        let addr = self.start();
        addr.get_extent(Block::align(addr)) >> Line::LOG_BYTES
    }

    /// Mark the line. This will update the side line mark table.
    pub fn mark(&self, state: u8) {
        debug_assert!(!super::BLOCK_ONLY);
        Self::MARK_TABLE.store_atomic::<u8>(self.start(), state, Ordering::Relaxed);
    }

    /// Atomically load the current line mark state.
    pub fn get_mark_state(&self) -> u8 {
        debug_assert!(!super::BLOCK_ONLY);
        Self::MARK_TABLE.load_atomic::<u8>(self.start(), Ordering::Relaxed)
    }

    /// Test line mark state.
    pub fn is_marked(&self, state: u8) -> bool {
        self.get_mark_state() == state
    }

    /// Mark all lines the object is spanned to.
    pub fn mark_lines_for_object<VM: VMBinding>(object: ObjectReference, state: u8) -> usize {
        debug_assert!(!super::BLOCK_ONLY);
        let start = object.to_object_start::<VM>();
        let end = start + VM::VMObjectModel::get_current_size(object);
        let start_line = Line::from_unaligned_address(start);
        let mut end_line = Line::from_unaligned_address(end);
        if !Line::is_aligned(end) {
            end_line = end_line.next();
        }
        let mut marked_lines = 0;
        let iter = RegionIterator::<Line>::new(start_line, end_line);
        for line in iter {
            if !line.is_marked(state) {
                marked_lines += 1;
            }
            line.mark(state)
        }
        marked_lines
    }

    /// Bulk set the local mark bits of a line range.
    ///
    /// This is useful during concurrent marking. By doing this, concurrent marking will
    /// conservatively consider all objects allocated in the line range as live, and the mutator
    /// doesn't need to explicitly mark bump-allocated objects in the fast path.
    pub fn initialize_mark_table_as_marked<VM: VMBinding>(lines: Range<Line>) {
        if lines.start < lines.end {
            let bytes = lines.end.start() - lines.start.start();
            VM::VMObjectModel::LOCAL_MARK_BIT_SPEC
                .extract_side_spec()
                .bset_metadata(lines.start.start(), bytes);
        }
    }

    pub fn inc_reuse_counts(lines: Range<Line>) {
        let mut l = lines.start;
        while l < lines.end {
            let addr = l.start();
            let count = IX_LINE_REUSE_COUNT.load_atomic::<u8>(addr, Ordering::SeqCst);
            let new_count = if count == u8::MAX { 0 } else { count + 1 };
            IX_LINE_REUSE_COUNT.store_atomic::<u8>(addr, new_count, Ordering::SeqCst);
            l = l.next();
        }
    }

    /// Bulk set line mark states.
    pub fn bulk_set_line_mark_states(line_mark_state: u8, lines: Range<Line>) {
        for line in RegionIterator::<Line>::new(lines.start, lines.end) {
            line.mark(line_mark_state);
        }
    }

    /// Eagerly mark all line mark states and all side mark bits in the gap.
    ///
    /// Useful during concurrent marking.
    pub fn eager_mark_lines<VM: VMBinding>(line_mark_state: u8, lines: Range<Line>) {
        Self::bulk_set_line_mark_states(line_mark_state, lines.clone());
        Self::initialize_mark_table_as_marked::<VM>(lines);
    }

    pub fn clear_field_unlog_table<VM: VMBinding>(lines: Range<Line>) {
        if lines.start < lines.end {
            let bytes = lines.end.start() - lines.start.start();
            VM::VMObjectModel::GLOBAL_FIELD_UNLOG_BIT_SPEC
                .as_spec()
                .extract_side_spec()
                .bzero_metadata(lines.start.start(), bytes);
        }
    }

    pub fn initialize_field_unlog_table_as_unlogged<VM: VMBinding>(lines: Range<Line>) {
        if lines.start < lines.end {
            let bytes = lines.end.start() - lines.start.start();
            VM::VMObjectModel::GLOBAL_FIELD_UNLOG_BIT_SPEC
                .as_spec()
                .extract_side_spec()
                .bset_metadata(lines.start.start(), bytes);
        }
    }
}

const LOG_BITS_PER_LINE: usize = Line::LOG_BYTES - rc::LOG_MIN_OBJECT_SIZE + rc::LOG_REF_COUNT_BITS;
const BITS_PER_LINE: usize = 1 << LOG_BITS_PER_LINE;
const BYTES_PER_LINE: usize = BITS_PER_LINE >> LOG_BITS_IN_BYTE as usize;
const _: () = assert!(BITS_PER_LINE >= 8 && BITS_PER_LINE % 8 == 0);

pub(super) struct RCArray<'a> {
    table: &'a [[AtomicU8; BYTES_PER_LINE]; Block::LINES],
}

impl RCArray<'_> {
    /// Borrow the `RC_TABLE` metadata bytes for `block` as an array of `AtomicU8` per line.
    ///
    /// # Safety
    /// - `crate::util::rc::RC_TABLE` side metadata must be mapped for `[block.start(), block.end())`.
    /// - No thread may perform non-atomic writes (`store`, `store_byte_relaxed`, `bzero_metadata`,
    ///   `bset_metadata`) to `RC_TABLE` for `block` for the lifetime `'a` of the returned `RCArray`
    ///   (concurrent atomic `RC_TABLE` updates via `AtomicU8` are sound).
    pub(super) unsafe fn of(block: Block) -> Self {
        debug_assert!(crate::util::rc::RC_TABLE.is_mapped(block.start()));
        let ptr = block
            .rc_table_start()
            .to_ptr::<[[AtomicU8; BYTES_PER_LINE]; Block::LINES]>();
        Self {
            // SAFETY: `ptr` has alignment 1 (`align_of::<AtomicU8>() == 1`), `RC_TABLE` is global
            // contiguous side metadata whose `[block.start(), block.end())` bytes are mapped and
            // zero-initialized by `MMAPPER`, and caller guarantees no concurrent non-atomic writes
            // for lifetime `'a`.
            table: unsafe { &*ptr },
        }
    }

    pub(super) fn is_dead(&self, i: usize) -> bool {
        self.table[i]
            .iter()
            .fold(0u8, |acc, byte| acc | byte.load(Ordering::Relaxed))
            == 0
    }
}
