# Step Analysis (auto-saved)

## Target
- File: `src/util/metadata/side_metadata/global.rs`
- Strategy: Investigate using `StwProof` for non-atomic `load`/`store`.

## Findings
- Line 622: `load` is unsafe because concurrent access is UB.
- Line 654: `store` is unsafe because concurrent access is UB.
- Both are used in contexts like `find_prev_non_zero_value` which is used in `vo_bit/mod.rs` for conservative stack scanning and heap iteration.
- Heap iteration (e.g. for JVM-TI) can happen during mutator time, where `StwProof` is not available.
- Therefore, requiring `StwProof` for these methods would break usages during mutator time.

## Attempted Changes
- None (analysis only).

## Blockers / Insights for Next Step
- I need to find another target for safe abstractions.
- I identified `src/util/metadata/vo_bit/mod.rs` as a potential target to investigate if we can use `StwProof` there or if it's also used during mutator time.
