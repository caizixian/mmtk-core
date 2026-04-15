# Step Analysis (auto-saved)

## Target
- File: `src/util/heap/layout/map32.rs`
- Strategy: Replace `UnsafeCell` + `Mutex` with `RwLock` to eliminate unsafe operations.

## Findings
- `Map32` uses `UnsafeCell<Map32Inner>` and `Mutex<()>` to manage concurrent access.
- `Deref` implementation uses `unsafe { &*self.inner.get() }` which bypasses the lock and is potentially unsound if called concurrently with mutation.
- `mut_self` and `mut_self_with_sync` use `unsafe` to get mutable references.
- Replacing this pattern with `RwLock<Map32Inner>` could eliminate most unsafe blocks in this file.

## Attempted Changes
- None yet.

## Blockers / Insights for Next Step
- Need to verify if replacing `Deref` with explicit read locks is feasible and doesn't break existing code or introduce deadlocks.
