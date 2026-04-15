# Step Analysis (auto-saved)

## Target
- File: `src/scheduler/mod.rs`, `src/scheduler/gc_work.rs`, `src/plan/global.rs` and all files implementing `Plan`.
- Strategy: Restrict `ExclusivePlanAccessProof` creation to the `scheduler` module and thread it through `schedule_collection` to enforce safe capability token usage.

## Findings
- `ExclusivePlanAccessProof::new()` is currently `pub(crate)` and not unsafe, allowing any file in the crate to manufacture it without audit.
- This violates Anti-pattern #10 (Manufacturing proof/capability tokens at every call site).
- The token is zero-sized and can be made `Copy`.
- `ScheduleCollection` work packet is created in `scheduler.rs` (which is in the `scheduler` module) and can be passed the token.
- `ScheduleCollection` can pass the token to `Plan::schedule_collection`.
- `Plan::schedule_collection` can pass the token to `Prepare` and `Release` work packets instead of them manufacturing it.

## Attempted Changes
- None yet. Planning to modify the trait and all implementations in this step to keep the build working.

## Blockers / Insights for Next Step
- Need to update 12 files implementing `schedule_collection`. This is a large edit but necessary to avoid breaking the build.
