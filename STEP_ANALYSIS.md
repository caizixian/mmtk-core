# Step Analysis (auto-saved)

## Target
- File: src/policy/immix/immixspace.rs and src/plan/immix/global.rs
- Strategy: Move work packet creation to schedule_collection to use &'static self.

## Findings
- Line 450 in immixspace.rs: `let space = unsafe { &*(self as *const Self) };` used for `PrepareBlockState` tasks.
- Line 547 in immixspace.rs: `let space = unsafe { &*(self as *const Self) };` used for `SweepChunk` tasks.
- Both require `&'static ImmixSpace<VM>`.
- `Immix::schedule_immix_full_heap_collection` in `src/plan/immix/global.rs` takes `immix_space: &ImmixSpace<VM>`. Changing it to `&'static ImmixSpace<VM>` allows passing `&self.immix_space` from `schedule_collection` where `self` is `&'static self`.
- `SweepChunk` in `immixspace.rs` is private and needs to be `pub(crate)` to be used in `global.rs`.

## Attempted Changes
- Made `SweepChunk` and `FlushPageResource` `pub(crate)` in `immixspace.rs`.
- Made `defrag` field `pub(crate)` in `ImmixSpace` to allow access to `mark_histograms`.
- Updated `schedule_immix_full_heap_collection` in `global.rs` to take `'static ImmixSpace` and schedule `PrepareBlockState` and `SweepChunk` tasks.
- Updated call sites in `Immix`, `GenImmix`, `StickyImmix`, and `ConcurrentImmix` to pass appropriate `UnlogBitsOperation`.
- Added `SweepChunk` task generation for nursery GC in `StickyImmix::schedule_collection`.
- Removed unsafe blocks and task generation from `ImmixSpace::prepare` and `release`, and removed `generate_sweep_tasks`.

## Blockers / Insights for Next Step
- Need to verify with `cargo check`.
