# Cached Line Mark States + Prefetch Exploration

## Summary

**Result: Neutral** (−0.65% on lusearch, within noise).

Optimization: cache `line_unavail_state` and `line_mark_state` in the allocation hot path, and software-prefetch the line mark table when acquiring reusable blocks. This eliminates 2 redundant `Ordering::Acquire` loads per hole search and warms the cache for blocks whose metadata may not be in L1.

## Approach

| Change | What it does |
|---|---|
| `snapshot_line_mark_states()` | Load both states once at the top of `acquire_recyclable_lines()` |
| `get_next_available_lines_with_states()` | Static method that accepts pre-loaded states, avoiding `&self` atomic loads |
| `MetadataByteArrayRef::as_slice()` | Exposes the raw byte array for prefetching |
| `_mm_prefetch` in `acquire_recyclable_block()` | Prefetches both cache lines (128 bytes) of the line mark table |

### Why neutral

On x86-64, `Ordering::Acquire` is just a regular `mov` instruction — the hardware memory model provides acquire semantics for free. The 2 saved loads per hole are ~1–2ns, vs the hole search itself at ~3ns and `memory::zero()` at 20–200ns per hole.

The prefetch has limited benefit because:
1. Sweep just wrote the line mark table, so it's often still warm in L2/L3.
2. Even if cold, the first byte load triggers a hardware prefetch that covers 64 bytes.

### Failed variant: struct field caching

An earlier attempt added `cached_unavail_state: u8` and `cached_mark_state: u8` fields to `ImmixAllocator`. This **crashed with SIGSEGV** because `ImmixAllocator` is `#[repr(C)]` and embedded in the `Mutator` struct — changing its size shifted field offsets, breaking the OpenJDK binding's hardcoded layout. The fix was to use local variables instead of struct fields.

## Benchmark Results

Commit `22856f0261` on branch `hole-simd`, baseline `before-simd-hole` (eeb0cc97).

| Benchmark | Baseline (ms) | Current (ms) | Diff |
|---|---|---|---|
| lusearch (5x heap) | 10370.8 ±636.1 | 10303.6 ±1048.6 | −0.65% |
| fop | — | 966.6 | no baseline |
| h2 | — | 3977.0 | no baseline |
| xalan | — | 5060.4 | no baseline |

## Conclusion

This optimization is **safe but not impactful**. The allocation path around hole search is already well-optimized on x86-64. The remaining cost is dominated by `memory::zero()` (20–200ns per hole) rather than the hole search mechanics (~3ns).

For meaningful allocation path improvements, focus should shift to:
- Reducing `memory::zero()` overhead (e.g., lazy zeroing, zeroing at sweep time)
- Reducing the number of holes (block selection heuristics)
- Higher-level allocation path changes (batch allocation, bump pointer alignment)
