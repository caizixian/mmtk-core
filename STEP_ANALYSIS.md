# Step Analysis (auto-saved)

## Target
- File: `src/util/rust_util/mod.rs`
- Strategy: Investigate `ProofCell::get_ref` and whether it can be made safe by threading a proof token.

## Findings
- Investigated `ProofCell::get_ref` (line 117). It returns a shared reference `&T` from an `UnsafeCell<T>`.
- It is marked `unsafe` because the caller must ensure no concurrent mutable accesses.
- Making it safe by threading a proof token (like `SharedPlanAccessProof`) would require passing the token to `get_plan()` and all its callers.
- `get_plan()` is used in over 50 places across the codebase, including hot paths like allocation.
- Threading a token to all these places would be a massive refactoring and might introduce overhead or complexity that violates the "zero-cost reads" requirement.
- Therefore, `ProofCell::get_ref` is confirmed as irreducible to maintain zero-cost reads on hot paths without massive refactoring.
- Also checked `GCWorkerShared` `Sync` impl in `worker.rs`. It is needed because it contains `WorkerLocalStat` which contains `WorkCounter` trait objects. `WorkCounter` only implies `Send`, not `Sync`, because one of its implementations (`WorkPerfEvent`) wraps a perf event handle that might not be thread-safe. Thus it is also irreducible.

## Attempted Changes
- None. All analyzed items were confirmed as irreducible.

## Blockers / Insights for Next Step
- The work queue item was completed (investigation).
- I am adding a new item to the work queue to investigate if `bytemuck` can be upgraded or worked around to remove the unsafe `Zeroable` impl in `atomic_box.rs`.
