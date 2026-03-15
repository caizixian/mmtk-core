//! Software prefetch utilities for GC tracing.
//!
//! Provides thin wrappers around architecture-specific prefetch intrinsics.
//! Used in the tracing loop to prefetch object headers ahead of processing,
//! hiding memory latency during pointer-chasing graph traversal.
//!
//! ## Two-Stage Prefetch Pipeline
//!
//! The tracing loop uses a two-stage pipeline to overlap memory latency:
//!
//! ```text
//! Time →  slot i              slot i+O            slot i+E
//!         ─────────           ─────────           ─────────
//! Edge PF for i+E:  ──────────────────────────────→ slot content in L1
//! Obj PF for i+O:   ──────────→ object header in L1
//! Process slot i:   ① ② ③ ④ ⑤ ⑥
//!                   ↑ uses data prefetched O iters ago
//!                   ↑↑ uses slot content prefetched E iters ago
//! ```
//!
//! 1. **Edge PF at i+E**: loads `slots[i+E]` to warm up slot content for
//!    future dereference by the object PF stage.
//! 2. **Object PF at i+O**: dereferences `slots[i+O]` (now in L1 thanks
//!    to edge PF from E−O iterations ago), prefetches the object header.
//!
//! Based on Huang 2025 and microbenchmark analysis:
//! combined E=32, O=16, NTA → 37.8% tracing speedup in microbenchmarks.

use crate::util::Address;

/// Prefetch distance for edge (slot content) in the `process_slots` loop.
/// At iteration `i`, we load `slots[i + EDGE_PREFETCH_DISTANCE]` to bring
/// the slot's content into L1 cache, feeding the object prefetch pipeline.
///
/// 32 provides ~800c of lookahead at ~25c per slot, enough to hide DRAM
/// latency. This is intentionally further ahead than OBJECT_PREFETCH_DISTANCE
/// so that by the time the loop reaches i+O for object prefetching, the slot
/// content (fetched by edge PF E−O=16 iterations earlier) is already in L1.
pub const EDGE_PREFETCH_DISTANCE: usize = 32;

/// Prefetch distance for object headers in the `process_slots` loop.
/// At iteration `i`, we load `slots[i + OBJECT_PREFETCH_DISTANCE]`,
/// dereference it to get the object reference, then prefetch the object's
/// header cache line (mark bits, forwarding word, compressed klass).
///
/// 16 provides ~400c of lookahead, enough to hide L3/DRAM misses on Zen 3.
/// See `benches/mock_bench/prefetch_tracing_analysis.md` for derivation.
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
