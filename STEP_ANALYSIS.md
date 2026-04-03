# Step Analysis (auto-saved)

## Target
- File: `src/plan/markcompact/gc_work.rs`
- Strategy: Remove raw pointer from work packet and use `get_plan_mut`

## Findings
- Line 35: `plan` is stored as `*const MarkCompact<VM>`.
- Line 47: `plan` is cast to `*mut` and dereferenced.
- Eliminable by removing `plan` from struct and using `mmtk.get_plan_mut()` and downcasting.

## Attempted Changes
- Removing `plan` field and `unsafe impl Send` from `UpdateReferences`.
- Using `unsafe { mmtk.get_plan_mut() }.downcast_mut::<MarkCompact<VM>>().unwrap()` in `do_work`.
- Updating `new` to take no arguments.
- Will also need to update call site in `src/plan/markcompact/global.rs`.

## Blockers / Insights for Next Step
- None so far.
