//! This module exposes private items in mmtk-core for testing and benchmarking. They must not be
//! used in production.
//!
//! # Notes on inlining
//!
//! In mmtk-core, we refrain from inserting inlining hints manually.  But we use `#[inline(always)]`
//! in this module explicitly because the functions here are simple wrappers of private functions,
//! and the compiler usually fails to make the right decision given that those functions are not
//! used often, and we don't compile the benchmarks using feedback-directed optimizations.

pub use crate::util::metadata::side_metadata::helpers::scan_non_zero_bits_in_metadata_bytes;
use crate::util::linear_scan::Region;
use crate::util::metadata::side_metadata::SideMetadataSpec;

use super::Address;

/// Expose `zero_meta_bits` when running `cargo bench`.
#[inline(always)]
pub fn zero_meta_bits(
    meta_start_addr: Address,
    meta_start_bit: u8,
    meta_end_addr: Address,
    meta_end_bit: u8,
) {
    SideMetadataSpec::zero_meta_bits(meta_start_addr, meta_start_bit, meta_end_addr, meta_end_bit)
}

/// Expose `set_meta_bits` when running `cargo bench`.
#[inline(always)]
pub fn set_meta_bits(
    meta_start_addr: Address,
    meta_start_bit: u8,
    meta_end_addr: Address,
    meta_end_bit: u8,
) {
    SideMetadataSpec::set_meta_bits(meta_start_addr, meta_start_bit, meta_end_addr, meta_end_bit)
}

// --- Immix constants for benchmarking ---
/// Number of lines in an Immix block (128 with default settings).
pub const BLOCK_LINES: usize = crate::policy::immix::block::Block::LINES;
/// Log2 of the bytes in an Immix line (8, i.e. 256 bytes).
pub const LINE_LOG_BYTES: usize = crate::policy::immix::line::Line::LOG_BYTES;
/// The initial line mark state.
pub const LINE_RESET_MARK_STATE: u8 = crate::policy::immix::line::Line::RESET_MARK_STATE;
/// The maximum line mark state before wrapping.
pub const LINE_MAX_MARK_STATE: u8 = crate::policy::immix::line::Line::MAX_MARK_STATE;

/// The work buffer capacity used by ProcessEdgesWork / VectorQueue.
pub const WORK_BUFFER_CAPACITY: usize = crate::scheduler::EDGES_WORK_BUFFER_SIZE;

#[cfg(feature = "mock_test")]
pub use crate::mmtk::MMAPPER;
