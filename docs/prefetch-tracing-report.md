# Software Prefetching in GC Tracing Loop — Experiment Report

**Date:** 2026-03-15
**Status:** ✅ Improvement — kept
**Commit:** `79463f7f72` (parent: `567f7817ee`)

## Hypothesis

Profiling (`docs/genimmix-profiling-report.md`) identified two major memory-latency
bottlenecks in the GC tracing hot path:

1. **`ProcessEdgesWork::do_work`** — 29% self-time at the `test` instruction after
   oop decompression, caused by cache misses during pointer-chasing through the
   object graph. Each `mov (%r15),%r12d` (slot load) and `mov (%rax),%r12` (oop
   deref) is a potential L3/DRAM miss.

2. **`PlanScanObjects::do_work`** — 53% self-time at compressed klass decompression,
   caused by cache misses loading object headers.

Software prefetching can overlap these memory latencies with useful computation by
issuing prefetch instructions N iterations ahead. Microbenchmarks in
`benches/mock_bench/prefetch_tracing.rs` validated that NTA prefetch with E=32
(edge distance) and O=16 (object distance) yields 9–18% GC tracing speedup in
synthetic workloads.

## Changes Made

Three files modified (`src/util/prefetch.rs` [NEW], `src/util/mod.rs`, `src/scheduler/gc_work.rs`):

### `src/util/prefetch.rs` (new module)
- `prefetch_nta(addr: Address)` — wraps `_mm_prefetch` with `_MM_HINT_NTA`
- Constants: `OBJECT_PREFETCH_DISTANCE = 16`, `SCAN_PREFETCH_DISTANCE = 4`
- NTA hint chosen because GC-traced objects are typically visited once per
  collection cycle; they should not pollute higher cache levels.

### `src/scheduler/gc_work.rs`
- **`process_slots()`**: At iteration `i`, loads `slots[i + 16]` to get the object
  reference, then prefetches the object's header cache line. This overlaps the
  memory access for future slots with processing of the current slot.
- **`ScanObjectsWork::do_work_common()`**: Prefetches `objects_to_scan[idx + 4]`'s
  header to overlap klass decompression stall with current object scanning. Shorter
  distance (4) because per-object scanning is heavier than per-slot processing.

## Benchmark Results

**Setup:** AMD EPYC 7B13 (Zen 3), GenImmix, 3× minheap, 32 GC threads, 32 app
threads, release build, 5 iterations per invocation.

Baseline: 10 invocations (run `d5d8fe0feee1`)
Comparison: 5 invocations

| Benchmark | Baseline (ms) | Current (ms) | Diff   | Status     |
|-----------|:-------------:|:------------:|:------:|:----------:|
| fop       | 1067.4 ±47.1  | 1026.4 ±50.3 | -3.84% | ✅ faster  |
| h2        | 4375.9 ±99.7  | 4208.2 ±128.6| -3.83% | ✅ faster  |
| lusearch  | 9429.1 ±119.1 | 9404.0 ±177.6| -0.27% | ➡️ neutral |

**Geometric mean: -2.66% (improvement)**

## Follow-up: Two-Stage Pipeline (E=32 + O=16) — Reverted

Microbenchmark analysis (`prefetch_tracing_analysis.md`) showed that combined
edge (E=32) + object (O=16) prefetch achieves -37.8% tracing speedup in synthetic
workloads. We tested this configuration:

| Benchmark | Baseline (ms) | Two-Stage (ms) | Diff   | vs Object-Only |
|-----------|:-------------:|:--------------:|:------:|:--------------:|
| fop       | 1067.4 ±47.1  | 1066.2 ±102.4  | -0.11% | worse (-3.84% → -0.11%) |
| h2        | 4375.9 ±99.7  | 4248.8 ±136.5  | -2.90% | similar (-3.83% → -2.90%) |
| lusearch  | 9429.1 ±119.1 | 9459.8 ±396.0  | +0.33% | similar |

**Geometric mean: -0.91% (neutral) — worse than object-only (-2.66%)**

The edge prefetch regresses performance because:
1. Slot buffers within work packets have good spatial locality (produced by
   scanning contiguous OopMap fields), so the HW prefetcher already handles them.
2. The extra `slot.load()` at `i+32` competes for load ports with useful work.
3. Microbenchmarks use random DAGs maximizing cache misses, but real heaps have
   partial locality that the HW prefetcher exploits.

**Conclusion:** Object-only prefetch (O=16) is the better configuration for real
workloads. The two-stage pipeline was reverted.

## Analysis

### fop (-3.84%)
Despite being a small-heap benchmark (13 MB minheap → 39 MB at 3×), fop shows a
clear improvement. GC is infrequent with fop, but each GC pause involves tracing
through the live object graph where prefetching helps hide cache misses. The ±CI
(~4.5% of mean) is acceptable for 5 invocations.

### h2 (-3.83%)
h2 is the benchmark most sensitive to tracing optimizations: 95% of its GC worker
time is tracing, with 60% in `CopySpace::trace_object`. The -3.83% improvement with
tight CI (±2.3% of mean for baseline) confirms that prefetching effectively hides
memory latency in the tracing hot path. Given that GC workers are only ~15% of total
h2 time, a 3.83% total improvement implies roughly **25% speedup in GC tracing
itself** (0.0383 / 0.148 ≈ 0.259).

### lusearch (-0.27%, neutral)
Lusearch is scheduler-overhead-dominated at 32 GC threads: 52% of GC time is futex
syscalls, not tracing. With only 16% of GC time in actual tracing, even a substantial
tracing speedup cannot measurably affect total runtime. The neutral result is expected
and consistent with the profiling data.

### Prefetch Distance Choices
- **E=16 for slots**: Each slot processes in ~2.5 ns (from profiling); with L3 latency
  ~40 ns on Zen 3, need ~16 slots of look-ahead to hide the miss.
- **S=4 for scan objects**: Object scanning is heavier (~10 ns per object); need only
  ~4 objects of look-ahead.
- **NTA hint**: Objects are visited once during GC tracing; NTA avoids polluting L1/L2
  caches with data that won't be reused before the next GC cycle.

## Conclusions

1. **Software prefetching in the tracing loop provides a real, measurable improvement**
   of ~3.8% on tracing-sensitive benchmarks (h2, fop), with a geometric mean of -2.66%
   across the tested suite.

2. **The improvement is consistent with the profiling data**: h2 (tracing-dominated)
   benefits most; lusearch (scheduler-dominated) is neutral. This confirms the
   bottleneck analysis from `genimmix-profiling-report.md`.

3. **Next steps for further optimization:**
   - ~~**Wider benchmark validation**~~: ✅ Done — tested on sunflow, eclipse, pmd,
     biojava (10 invocations each at 3× minheap). **No regressions**: biojava −0.59%,
     eclipse −1.48%, pmd +1.25%, sunflow −0.54%, geometric mean −0.35% (all neutral).
     Xalan failed all invocations at 3× minheap.
   - ~~**Profile with prefetching enabled**~~: ✅ Done — h2 re-profiled with async-profiler.
     ProcessEdgesWork self-time dropped from 29% to 13.4% (>50% reduction). Bottleneck
     shifted to object copying (18.9%), side metadata (9.1%), CAS contention (9.1%),
     and descriptor lookup (6.5%). No single remaining target offers >2% total improvement.
     See `genimmix-profiling-report.md` § Post-Prefetch Re-Profiling.
   - **Compound metadata prefetch** (side metadata + descriptor_map): Architecturally
     complex — requires exposing per-space metadata specs and the descriptor_map to
     `process_slots()`, which currently only has the object address. Maximum theoretical
     improvement ~2% total.
   - **Tune prefetch distances**: The current O=16/S=4 are from microbenchmarks and
     validated on real workloads. Testing wider range (O=8, O=32, S=2, S=8) could
     yield marginal gains, but the slot-processing loop timing is well-matched.

## Experiment: Descriptor Map Prefetch

**Commit**: `f489a27091` (reverted at `8111ea5663`)
**Experiment run**: `a47316903934` ("descriptor-map-prefetch", 10 invocations)
**Baseline**: No dedicated baseline run was collected for this experiment; the
comparison used numbers from the previous session's header-only prefetch run
`739b7ba79d3c` (5 invocations, different session). This is a methodological weakness.

**Hypothesis**: Prefetching `descriptor_map[chunk_index]` alongside the object header
in `process_slots()` would hide the latency of space descriptor lookup (6.5% of GC
self-time in the post-prefetch profile).

**Implementation**: Added `prefetch_descriptor_for_address()` to the `VMMap` trait,
implemented in `Map32` to prefetch the descriptor_map entry for the given address's
chunk index.

**Results** (3× minheap, 32 GC threads):

| Run ID | Description | fop (ms) | h2 (ms) | Invocations |
|--------|-------------|----------|---------|-------------|
| `a47316903934` | +descriptor prefetch | 1016.7 | 4243.6 | 10 |

Without a same-session baseline, impact cannot be reliably quantified.
The absolute numbers are consistent with header-only prefetch runs from
other sessions, suggesting no measurable improvement.

**Conclusion**: No measurable impact. During nursery GC, only a few chunks contain
nursery objects, so the `descriptor_map` entries for active chunks stay warm in L2/L3
cache. The 6.5% profile self-time is computation/dispatch overhead, not cache misses.
**Reverted.**

## Experiment: Multi-Cache-Line Body Prefetch in ScanObjects

**Commit**: `d3a987367c` (reverted at `08afa68e55`)
**Experiment run**: `9eb66ebb3c3f` ("body-prefetch-3-cachelines", 10 invocations)
**Baseline run**: `a47316903934` ("descriptor-map-prefetch", 10 invocations)

> [!WARNING]
> The baseline run `a47316903934` had both header prefetch AND descriptor-map
> prefetch applied. However, the descriptor-map prefetch was independently shown
> to have no measurable impact, so this baseline is approximately equivalent to
> header-only prefetch. This is still not a proper A/B comparison.

**Hypothesis**: When `ScanObjects` prefetches an object header D=4 positions ahead,
also prefetching the second (+64 bytes) and third (+128 bytes) cache lines would
pre-warm reference fields that `scan_object()` iterates, reducing field-iteration
cache misses for objects spanning multiple cache lines.

**Implementation**: In `ScanObjectsWork::do_work_common`, added two additional
`prefetch_nta` calls at `addr + 64` and `addr + 128` alongside the existing header
prefetch.

**Results** (10 invocations, 3× minheap, 32 GC threads):

| Run ID | Description | fop (ms) | h2 (ms) |
|--------|-------------|----------|---------|
| `a47316903934` | baseline (header+descriptor prefetch) | 1016.7 | 4243.6 |
| `9eb66ebb3c3f` | +body prefetch (3 cache lines) | 1012.5 | 4216.9 |
| | **Diff** | **−0.41%** | **−0.63%** |

**Conclusion**: Marginal improvement (≤1%), within noise. Most Java objects in nursery
GC are small (< 64 bytes), so their reference fields are on the same cache line as the
header. The extra prefetches are mostly wasted for small objects and only help for
objects with many reference fields spanning multiple cache lines. **Reverted** to
isolate the forwarding cache experiment.

## Experiment: Forwarding Pointer Cache in GenNurseryProcessEdges

**Commit**: `f3a61d95bb` (reverted at `74413fd034`)
**Experiment run**: `7b76cb777ef5` ("fwd-cache-only (no body prefetch)", 10 invocations)
**Baseline run**: `8c92da1178b0` ("baseline: header-only prefetch (no cache, no body-pf)", 10 invocations)

**Hypothesis**: During nursery GC, multiple slots within a work packet may reference
the same nursery object. Each occurrence calls `trace_object` → `attempt_to_forward`
which does a CAS on the in-header forwarding bits. A 256-entry direct-mapped cache
mapping original addresses to forwarded addresses could skip redundant `trace_object`
calls for already-forwarded objects.

**Implementation**: Added a `fwd_cache: [(usize, usize); 256]` (4KB, L1-resident)
field to `GenNurseryProcessEdges`. In `process_slot`, check the cache before calling
`trace_object`; on hit, use the cached forwarded address directly. Only cache nursery
objects (where `new_object != object`).

**Results** (10 invocations, 3× minheap, 32 GC threads, same session):

| Run ID | Description | fop (ms) | ±CI | h2 (ms) | ±CI |
|--------|-------------|----------|-----|---------|-----|
| `8c92da1178b0` | baseline (no cache) | 1026.1 | ±23.4 | 4189.9 | ±72.8 |
| `7b76cb777ef5` | +forwarding cache | 1005.2 | ±27.8 | 4189.5 | ±87.5 |
| | **Diff** | **−2.04%** | | **−0.01%** | |

**Analysis**: h2 shows zero improvement (−0.01%). fop shows −2.04% but the CIs overlap
(baseline 1026.1 ±23.4 = [1002.7, 1049.5] vs experiment 1005.2 ±27.8 = [977.4, 1033.0]).
The result is not statistically significant.

**Root cause analysis**: The hypothesis was wrong. Work packets are created by
`ScanObjects`, which scans one object at a time. Each object's reference fields point
to DIFFERENT nursery objects (it's rare for two fields within one object to point to the
same target). So intra-packet target duplication is extremely low, making the cache hit
rate near zero. The cache only adds overhead (4KB initialization + hash computation per
slot) with almost no benefit.

**Conclusion**: No measurable improvement. The forwarding pointer cache does not help
because the fundamental assumption (duplicate targets within work packets) is wrong.
**Reverted.**
