# Step Analysis (auto-saved)

## Target
- File: `src/util/alloc/allocator.rs`
- Strategy: Replace `RefCell` with `Mutex` in `AllocationOptionsHolder` to eliminate `unsafe impl Sync`.

## Findings
- Line 110: `unsafe impl Sync for AllocationOptionsHolder {}` is used because `RefCell` is not `Sync`.
- The type is only used by a single thread at a time (mutator or GC worker), but Rust requires `Sync` because it is shared across allocators in an `Arc`.
- Replacing `RefCell` with `Mutex` will make it safe and eliminate the `unsafe impl Sync`.

## Attempted Changes
- Planning to replace `RefCell` with `Mutex` and update call sites.

## Blockers / Insights for Next Step
- None so far.
