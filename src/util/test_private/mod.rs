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

// --- Mark bit / tracing helpers for benchmarking ---
pub use crate::util::metadata::mark_bit::MarkState;

#[cfg(feature = "mock_test")]
pub use crate::mmtk::MMAPPER;

// --- Re-exports for mock benchmarks (need real MMTk infrastructure) ---
#[cfg(feature = "mock_test")]
pub use crate::policy::immix::block::{Block, BlockState};
#[cfg(feature = "mock_test")]
pub use crate::policy::immix::defrag::Histogram;

/// Create a new zeroed histogram for use in sweep benchmarks.
#[cfg(feature = "mock_test")]
pub fn new_histogram() -> Histogram {
    [0; (crate::policy::immix::block::Block::LINES >> 1) + 1]
}
#[cfg(feature = "mock_test")]
pub use crate::policy::immix::line::Line;
#[cfg(feature = "mock_test")]
pub use crate::policy::immix::ImmixSpace;
#[cfg(feature = "mock_test")]
pub use crate::util::linear_scan::Region as RegionTrait;

// --- Object forwarding wrappers for mock benchmarks ---
// object_forwarding module is pub(crate), so we wrap individual functions.
#[cfg(feature = "mock_test")]
#[inline(always)]
pub fn attempt_to_forward<VM: crate::vm::VMBinding>(
    object: crate::util::ObjectReference,
) -> u8 {
    crate::util::object_forwarding::attempt_to_forward::<VM>(object)
}

#[cfg(feature = "mock_test")]
#[inline(always)]
pub fn get_forwarding_status<VM: crate::vm::VMBinding>(
    object: crate::util::ObjectReference,
) -> u8 {
    crate::util::object_forwarding::get_forwarding_status::<VM>(object)
}

#[cfg(feature = "mock_test")]
#[inline(always)]
pub fn clear_forwarding_bits<VM: crate::vm::VMBinding>(
    object: crate::util::ObjectReference,
) {
    crate::util::object_forwarding::clear_forwarding_bits::<VM>(object);
}

// --- Plan access helper ---
/// Get the ImmixSpace from an MMTK instance running the Immix plan.
#[cfg(feature = "mock_test")]
pub fn get_immix_space<VM: crate::vm::VMBinding>(
    mmtk: &crate::MMTK<VM>,
) -> &ImmixSpace<VM> {
    let plan = mmtk.get_plan();
    let immix = plan
        .downcast_ref::<crate::plan::immix::Immix<VM>>()
        .expect("Plan is not Immix");
    &immix.immix_space
}

// --- Tracing benchmark types (scheduler-based) ---
#[cfg(feature = "mock_test")]
pub use crate::scheduler::gc_work::SFTProcessEdges;
#[cfg(feature = "mock_test")]
pub use crate::scheduler::gc_work::ProcessEdgesWorkRootsWorkFactory;
#[cfg(feature = "mock_test")]
pub use crate::scheduler::gc_work::PlanProcessEdges;
#[cfg(feature = "mock_test")]
pub use crate::scheduler::gc_work::ProcessEdgesWorkTracerContext;
#[cfg(feature = "mock_test")]
pub use crate::scheduler::GCWorker;
#[cfg(feature = "mock_test")]
pub use crate::plan::marksweep::MarkSweep;
#[cfg(feature = "mock_test")]
pub use crate::policy::gc_work::DEFAULT_TRACE;

/// Trigger a GC request (force=true) WITHOUT calling block_for_gc.
/// This avoids a deadlock in MockVM: the mock! macro holds the MOCK_VM_INSTANCE mutex
/// for the duration of each mock call. If block_for_gc waits on a condvar while holding
/// this mutex, then stop_all_mutators (on the worker thread) cannot acquire the same mutex,
/// causing a deadlock. By splitting the trigger from the wait, the test can synchronize
/// outside the mock mutex.
///
/// Returns true if the GC was requested.
#[cfg(feature = "mock_test")]
#[inline(always)]
pub fn trigger_gc_no_block<VM: crate::vm::VMBinding>(
    mmtk: &crate::MMTK<VM>,
) -> bool {
    mmtk.gc_trigger.handle_user_collection_request(true, false)
}
