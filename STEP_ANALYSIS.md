# Step Analysis (auto-saved)

## Target
- File: `src/util/heap/layout/map32.rs`
- Strategy: Replace `UnsafeCell<Map32Inner>` with `Mutex<Map32Inner>` to eliminate unsafe interior mutability and raw pointer dereferences.

## Findings
- `Map32` uses `UnsafeCell<Map32Inner>` and a separate `Mutex<()>` for synchronization in some methods, but allows unsafe access via `Deref` and `mut_self`.
- This leads to unsafe trait impls for `Send` and `Sync` and several unsafe blocks for pointer dereferencing.
- Replacing `UnsafeCell` with `Mutex` will allow removing these unsafe operations and trait impls.
- `IntArrayFreeList` is `Send`, so `Map32Inner` is `Send`, making `Mutex<Map32Inner>` both `Send` and `Sync` automatically.

## Attempted Changes
- None yet. Plan to replace `UnsafeCell` with `Mutex` and update all call sites.

## Blockers / Insights for Next Step
- None anticipated. Need to verify that performance is not adversely affected, but these operations are likely not on the hot path.
