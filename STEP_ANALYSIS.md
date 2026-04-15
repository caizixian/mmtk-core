# Step Analysis (auto-saved)

## Target
- File: src/mmtk.rs
- Strategy: Eliminate unsafe lifetime extension for `static_plan` and remove redundant unsafe blocks.

## Findings
- Line 185: `let static_plan: &'static dyn Plan<VM = VM> = unsafe { &*(&*plan as *const _) };`
  - Attempted to remove by refactoring `GCTrigger` to not store plan.
  - Failed because `Space` calls `poll` and cannot provide `plan`.
  - Reverted changes.
- Line 436: `unsafe { &**self.plan.get_ref() }`
  - `InitializeOnce::get_ref` is a safe method!
  - The unsafe block is redundant.

## Attempted Changes
- Refactored `GCTrigger` and call sites to pass `plan` as argument (failed and reverted).
- Removed redundant unsafe block in `MMTK::get_plan` (line 436).

## Blockers / Insights for Next Step
- The `'static` reference hack in `src/mmtk.rs:185` is blocked by architectural constraints unless we refactor `Space` or find another way to share the plan.
- Successfully reduced unsafe count by 1 by removing redundant block.
