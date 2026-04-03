# Step Analysis (auto-saved)

## Target
- File: `src/scheduler/gc_work.rs`
- Strategy: Remove `worker` raw pointer from `ProcessEdgesBase` and pass it as a parameter to `ProcessEdgesWork` methods to eliminate unsafe blocks and unsafe `Send` impl.

## Findings
- `ProcessEdgesBase` stores a `*mut GCWorker` to allow `trace_object` and other methods to access the worker without passing it as a parameter.
- This leads to `unsafe { &mut *self.worker }` in `worker()` and `unsafe impl Send for ProcessEdgesBase`.
- We can eliminate this by changing `ProcessEdgesWork` trait methods to take `&mut GCWorker` as a parameter.
- This will affect 5 implementers: `SFTProcessEdges`, `PlanProcessEdges`, `UnsupportedProcessEdges` in `src/scheduler/gc_work.rs`, `SanityGCProcessEdges` in `src/util/sanity/sanity_checker.rs`, and `ProcessRootSlots` in `src/plan/concurrent/concurrent_marking_work.rs`.
- This is a high-impact refactoring that reduces unsafe count and improves safety.

## Attempted Changes
- Modified `src/scheduler/gc_work.rs` to update `ProcessEdgesWork` trait and implementers.
- Modified `src/util/sanity/sanity_checker.rs` to update `SanityGCProcessEdges`.
- Modified `src/plan/concurrent/concurrent_marking_work.rs` to update `ProcessRootSlots`.
- Removed `worker` from `ProcessEdgesBase` and added it to `ProcessEdgesWorkTracer` as a raw pointer (localized).


## Blockers / Insights for Next Step
- Need to ensure all implementers are updated to avoid build failure.
