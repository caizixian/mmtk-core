# Non-Temporal Zeroing Experiment Report

**Date:** 2026-03-15  
**Status:** ❌ No improvement — reverted  
**Commit:** `51d3f7972c` (reverted in `567431bdc3`)

## Hypothesis

Based on Blackburn et al., "Fast Conservative Garbage Collection" (OOPSLA 2011),
replacing `memset` zeroing with non-temporal stores (`movntdq`) should bypass the
CPU cache hierarchy, doubling effective zeroing bandwidth and reducing cache pollution.

Profiling (see `genimmix-profiling-report.md`) showed `__memset_avx2_unaligned_erms`
accounted for **81.2% of H2's allocation slow path** and **43.5% of lusearch's**.

## Changes Made

1. **`src/util/memory.rs`**: Added `nontemporal_zero()` using SSE2 `_mm_stream_si128`
   with 128-byte unrolled loop, alignment handling, <256B threshold, and `_mm_sfence`.
   Falls back to regular `zero()` on non-x86_64.

2. **`src/policy/space.rs:229`**: Page acquisition zeroing changed from
   `memory::zero()` → `memory::nontemporal_zero()`.

3. **`src/util/alloc/immix_allocator.rs:253`**: Recyclable line zeroing changed from
   `memory::zero()` → `memory::nontemporal_zero()`.

## Benchmark Results

10 invocations, GenImmix, 3× heap, AMD EPYC 7B13 (Zen 3, 128 cores):

| Benchmark | Baseline (ms) | NT Zeroing (ms) | Diff | Status |
|-----------|---------------|-----------------|------|--------|
| fop       | 3723.4 ±56.9  | 3749.1 ±41.2    | +0.69% | neutral |
| h2        | 5278.6 ±131.8 | 5300.1 ±89.4    | +0.41% | neutral |
| lusearch  | 19020.1 ±624.1| 19656.5 ±1160.8 | +3.35% | ❌ slower |

**Geometric mean: +1.47% (neutral, trending negative)**

## Analysis

### Why NT zeroing didn't help on this hardware

1. **ERMS already uses NT stores internally**: On AMD Zen 3 (and Intel since Ivy Bridge),
   `rep stosb` with Enhanced REP MOVSB/STOSB (ERMS) already uses non-temporal stores
   for large fills. The glibc `__memset_avx2_unaligned_erms` dispatches to `rep stosb`
   above a threshold, so our explicit `movntdq` was redundant.

2. **Recyclable lines are accessed immediately**: For the Immix recyclable lines path,
   zeroed memory is bump-allocated into almost immediately. NT stores force data out of
   L1, requiring it to be fetched back from memory on the first allocation.

3. **128-bit vs 256/512-bit width**: Our SSE2 `movntdq` uses 128-bit stores, while
   `memset` on this CPU uses 256-bit AVX2 or wider operations — 2× more instructions
   for the same work.

4. **Paper's hardware was 2011-era**: The paper tested on Intel i7-2600 (Sandy Bridge)
   where `memset` did NOT use NT stores. The benefit has been absorbed into modern
   standard libraries.

## Conclusions

- On modern CPUs with ERMS, explicit non-temporal zeroing provides no benefit over
  optimized `memset` implementations.
- The Immix recyclable lines path should NOT use NT stores since memory is used
  immediately after zeroing.
- More promising zeroing optimizations:
  - **Concurrent zeroing** (background thread) — moves zeroing off the critical path
  - **`dzmmap`** — skip explicit zeroing, let the kernel lazily zero pages
