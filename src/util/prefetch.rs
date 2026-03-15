//! Software prefetch utilities for GC tracing.
//!
//! Provides thin wrappers around architecture-specific prefetch intrinsics.
//! Used in the tracing loop to prefetch object headers ahead of processing,
//! hiding memory latency during pointer-chasing graph traversal.
//!
//! Based on Huang 2025 findings: edge=32, object=16, NTA → 9–18% GC speedup.

use crate::util::Address;

/// Prefetch distance for object headers in the `process_slots` loop.
/// At iteration `i`, we load `slots[i + OBJECT_PREFETCH_DISTANCE]` and
/// prefetch the resulting object's header.
///
/// 16 is the sweet spot from microbenchmarks (see `benches/mock_bench/prefetch_tracing.rs`):
/// enough to hide L3 latency (~40ns on Zen 3) given ~2.5ns per-slot processing.
pub const OBJECT_PREFETCH_DISTANCE: usize = 16;

/// Prefetch distance for objects in the `ScanObjectsWork::do_work_common` loop.
/// Smaller distance because per-object scanning work is heavier than per-slot.
pub const SCAN_PREFETCH_DISTANCE: usize = 4;

/// Prefetch a cache line for reading using Non-Temporal Access (NTA) hint.
///
/// NTA tells the CPU the data is unlikely to be reused soon, so it should be
/// placed in the outermost cache level (or bypass caches on some microarchitectures).
/// This is appropriate for GC tracing where objects are visited once per collection.
#[inline(always)]
pub fn prefetch_nta(addr: Address) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        std::arch::x86_64::_mm_prefetch(
            addr.to_ptr::<i8>(),
            std::arch::x86_64::_MM_HINT_NTA,
        );
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = addr; // suppress unused warning
    }
}
