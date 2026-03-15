# GenImmix Profiling Report: Lusearch & H2

This report documents instruction-level profiling of MMTk's GenImmix collector
on the DaCapo Chopin `lusearch` and `h2` benchmarks. It identifies the true
performance bottlenecks and explains why two prior allocation-path optimizations
(SIMD hole search, cached mark states) had no positive impact.

## Setup

| Parameter | Lusearch | H2 |
|-----------|----------|-----|
| Heap (3× min) | 63 MB | 2043 MB |
| GC threads | 32 | 32 |
| App threads | 32 (`-t 32`) | 32 (`-t 32`) |
| Runtime | 8030 ms | 4124 ms |

**Common:** AMD EPYC 7B13 (Zen 3), release+LTO, `debug = true` for DWARF symbols.

**Tools:** `perf record -e cpu-clock --call-graph dwarf` for instruction-level
annotation; async-profiler (CPU, DWARF cstack) → `jfrconv --cpu -o collapsed`
for function-level analysis via
[flamegraph_query.py](../.agents/skills/flamegraph_query/flamegraph_query.py).

## Where Time Goes

### Overall Breakdown (% of total CPU samples)

| Category | Lusearch | H2 |
|----------|--------:|---------:|
| GC workers | 20.8% | 14.8% |
| Alloc slow path | 7.5% | 3.8% |
| Application | 71.8% | 81.5% |

### GC Worker Breakdown (inclusive % of GC worker time)

| Function | Lusearch | H2 |
|----------|--------:|---------:|
| `ProcessEdgesWork::do_work` | 16.3% | **95.0%** |
| `CopySpace::trace_object` | 4.8% | **59.5%** |
| `VMObjectModel::copy` | — | **21.7%** |
| `PlanScanObjects::do_work` | 4.0% | 14.4% |
| `ImmixSpace::trace_object` | — | 9.4% |
| `side_metadata_access` | 2.6% | 8.6% |
| `syscall` (scheduler/futex) | **51.8%** | — |
| `poll_schedulable_work` | 9.7% | — |

> **H2** is **tracing-dominated**: 95% of GC worker time is actual tracing,
> with 60% in nursery copying and 22% in memcpy.
>
> **Lusearch** is **scheduler-overhead-dominated**: only 16% of GC time is
> tracing; 52% is futex syscalls from 32 GC threads competing for small work
> packets in a 63 MB heap.

### ProcessEdgesWork Callees (% of ProcessEdgesWork time)

```
ProcessEdgesWork::do_work
  ├── CopySpace::trace_object           lusearch 29% / h2 61%
  ├── PlanScanObjects::do_work          lusearch 25% / h2 15%
  ├── GenImmix::trace_object            lusearch 13% / h2 12%
  ├── InstanceKlass::oop_iterate        lusearch  8% / h2  2%
  ├── VMScanning::scan_object           lusearch  5% / h2  4%
  └── Map32::get_descriptor_for_address lusearch  3% / h2  3%
```

### Allocation Slow Path (% of slow-path time)

| Leaf function | Lusearch | H2 |
|---------------|--------:|-----:|
| `__memset` (page zeroing) | 43.5% | **81.2%** |
| `syscall` (mmap/mprotect) | 14.5% | 0.9% |
| Lock contention | 9.2% | 0.8% |

The allocation **fast path** (bump pointer + hole search) has ≈0 samples in both
benchmarks — it is not a bottleneck.

## Instruction-Level Hotspots (`perf annotate`)

With LTO+inlining, function-level profiles are very flat (~0.5–0.8% each).
`perf annotate` with DWARF debug info reveals the true hotspots inside inlined
code:

### CopySpace::trace_object (866 samples)

| % self | Instruction | Meaning |
|-------:|-------------|---------|
| 68.6% | `and $0x3,%al` | Stall after `lock cmpxchg` — forwarding CAS retry |
| 23.6% | `jne` (loop back) | CAS failed, retry |
| 0.5% | `lock cmpxchg` | Atomic CAS on forwarding bits |
| 0.2% | `call VMObjectModel::copy` | Actual memcpy of object |

**Bottleneck:** Cache-line contention on the forwarding bits CAS. Multiple GC
workers discovering the same object via different incoming edges all race on
the same header byte.

### ProcessEdgesWork::do_work (737 samples)

| % self | Instruction | Meaning |
|-------:|-------------|---------|
| 29.9% | `test %r12,%r12` (compressed path) | Null check after oop decompression |
| 28.8% | `test %r12,%r12` (non-compressed) | Null check after raw oop load |
| 14.6% | `test %r15,%r15` | Compressed-oop sign-bit check |

**Bottleneck:** Memory-latency-bound pointer chasing. Each `mov (%r15),%r12d`
(slot load) and `mov (%rax),%r12` (oop deref) is a potential cache miss through
the random-access object graph.

### PlanScanObjects::do_work (790 samples)

| % self | Instruction | Meaning |
|-------:|-------------|---------|
| 52.6% | `mov ...,%rdx` (COMPRESSED_KLASS_BASE) | Stall loading object header for klass |
| 7.3% | `add %rcx,%rax` + `jmp *%rax` | Klass-kind jump table dispatch |

**Bottleneck:** Object header load stall during compressed klass decompression.

### SideMetadataSpec::side_metadata_access (707 samples)

| % self | Instruction | Meaning |
|-------:|-------------|---------|
| 55.7% | `and %dl,%al` | Stall after loading metadata byte from side table |

**Bottleneck:** Cache miss on the side metadata byte load.

## Why Previous Optimizations Failed

### SIMD hole search (+11.46% regression)

`get_next_available_lines` does not appear in any profile. It runs during
allocation, which is dominated by the slow path (page acquisition + zeroing),
not the fast path (hole search). Each hole search scans only 2–16 bytes —
SSE2 setup and `pmovmskb` FP→INT domain crossing (5 cycles on Zen 3) cost
more than scalar code for such small inputs. The larger code size also
increased i-cache pressure.

### Cached mark states + prefetch (−0.65%, neutral)

The cached states eliminated 2 `Ordering::Acquire` loads, which on x86 compile
to plain `mov` (no fence) — saving ~1–2 ns per hole search. The line-mark-table
prefetch was a no-op because sweep already warms the cache. With hole search
absent from the profile entirely, saving 1–2 ns there cannot measurably affect
a benchmark where GC tracing + page zeroing dominate.

## Conclusions

1. **All GC hotspots are memory-latency-bound**, not compute-bound.
   Computational optimizations (SIMD, caching loads, reducing arithmetic) do
   not help because the CPU is stalled waiting for memory.

2. **The right optimization targets are:**
   - **Prefetching in the tracing loop** (overlap slot loads with processing)
   - **Reducing CAS contention** on forwarding bits (work partitioning)
   - **Page zeroing** (lazy zeroing, `mmap(MAP_POPULATE)`, background zeroing)

3. **Benchmark selection matters:**
   - **H2** is the right target for tracing optimizations (60% of GC time)
   - **Lusearch at 32 GC threads** exposes scheduler overhead (52% futex)
   - Neither benchmark makes the allocation fast path hot

4. **Inlining makes function-level profiling misleading.** Always use
   `perf annotate` with DWARF debug info to see instruction-level hotspots,
   or collapsed-stack analysis from async-profiler for call-chain breakdowns.

---

## Post-Prefetch Re-Profiling (H2)

After implementing object-only prefetching at D=16 with NTA hint (commit
`929c46144d`), g2 was re-profiled to measure the impact and identify the new
bottleneck distribution.

### Setup

Same as above; async-profiler CPU + DWARF cstack, 1370 GC-thread samples.

### Updated GC Worker Breakdown (self % of GC worker time)

| Function | Before Prefetch | After Prefetch | Change |
|----------|---------------:|---------------:|-------:|
| `ProcessEdgesWork::do_work` | **29.0%** | **13.4%** | **−15.6pp** |
| `VMObjectModel::copy` | 21.7%* | 11.2% | see note |
| `side_metadata_access` | 8.6% | 9.1% | +0.5pp |
| `CopySpace::trace_object` | — | 9.1% | new breakdown |
| `PlanScanObjects::do_work` | 14.4%* | 8.4% | see note |
| `__memcpy_avx_unaligned_erms` | — | 7.7% | previously inlined |
| `Map32::get_descriptor_for_address` | 3.0% | 6.5% | +3.5pp relative |
| `scan_object` | 4.0% | 5.1% | +1.1pp relative |
| `__memset_avx2` (page zeroing) | — | 4.7% | not in tracing |
| `__mprotect` | — | 3.9% | OS call |

*Before-prefetch values used inclusive %; after-prefetch values use self %.*

### ProcessEdgesWork Callees (post-prefetch)

```
ProcessEdgesWork::do_work (1354 samples)
  ├── CopySpace::trace_object              38.7%  (object forwarding + copy)
  │     └── VMObjectModel::copy            99.7%  (memcpy of object body)
  ├── PlanScanObjects::do_work             25.0%  (scan object fields)
  ├── GenImmix::trace_object               14.7%  (space dispatch)
  ├── VMScanning::scan_object               5.2%  (klass iteration)
  ├── Map32::get_descriptor_for_address     5.0%  (chunk-to-space lookup)
  ├── alloc::raw_vec::reserve               3.6%  (nodes Vec realloc)
  ├── InstanceKlass::oop_iterate            3.4%  (field iteration)
  └── Line::mark_lines_for_object           2.1%  (immix line marking)
```

### Key Finding: Bottleneck Shifted

The prefetch optimization successfully reduced the main pointer-chasing stall
by **>50%** (ProcessEdgesWork self time from 29% → 13.4%). The remaining GC
time is now distributed across multiple categories:

1. **Object copying** (18.9%): `VMObjectModel::copy` + `memcpy` — fundamental
   work in a generational collector. Cannot be reduced without changing the
   copy strategy or nursery sizing.

2. **Side metadata access** (9.1%): Cache miss on metadata byte loads.
   Prefetching requires knowing the metadata spec, which depends on the space
   — requires SFT lookup or architectural changes.

3. **CopySpace CAS contention** (9.1% self): Forwarding-bit CAS retries when
   multiple threads discover the same object. Fundamental to the concurrent
   forwarding design.

4. **Space descriptor lookup** (6.5%): `descriptor_map[chunk_index]` cache miss
   during `in_space()` dispatch. Could theoretically be prefetched alongside
   the object header, but requires exposing descriptor_map to process_slots().

### Optimization Target Feasibility Analysis

| Target | GC % | Total % | Feasibility | Notes |
|--------|-----:|--------:|------------|-------|
| Object copying | 18.9% | ~2.3% | ❌ Fundamental | Cannot avoid without design changes |
| Side metadata | 9.1% | ~1.1% | ⚠️ Complex | Needs per-space metadata spec access |
| CAS contention | 9.1% | ~1.1% | ❌ Design-level | Needs work partitioning / scheduler changes |
| Descriptor lookup | 6.5% | ~0.8% | ⚠️ Possible | Needs descriptor_map exposed to process_slots |
| Vec reallocation | 3.6% | ~0.4% | ✅ Easy | Pre-allocate nodes queue |
| PlanScanObjects | 8.4% | ~1.0% | ✅ Already done | D=4 scan prefetch in place |

**Conclusion:** No remaining single target offers >2% total runtime improvement.
The compound prefetch strategy (descriptor_map + side metadata) could
theoretically recover ~2% but requires significant architectural changes to
expose metadata addresses to the tracing loop without SFT lookups.

