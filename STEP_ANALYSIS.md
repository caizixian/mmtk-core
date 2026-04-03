# Step Analysis (auto-saved)

## Target
- File: src/util/heap/gc_trigger.rs, src/mmtk.rs
- Strategy: Remove plan storage from GCTrigger to eliminate unsafe static plan hack.

## Findings
- src/mmtk.rs:239: unsafe cast of plan to &'static dyn Plan. This is done to set the plan in GCTrigger.
- GCTrigger stores plan in a OnceLock<&'static dyn Plan>.
- Methods in GCTrigger that use plan can be refactored to take plan as an argument, as the plan is available at all call sites.
- This will allow removing the plan field from GCTrigger and the unsafe cast in mmtk.rs.

## Attempted Changes
- None yet.

## Blockers / Insights for Next Step
- Need to update GCTrigger methods and all call sites.

