# Step Analysis (auto-saved)

## Target
- File: `src/policy/copyspace.rs`
- Strategy: Remove unsafe in `rebind` by changing `prepare_worker` in `Plan` trait to take `&'static self`.

## Findings
- `src/policy/copyspace.rs:357`: `CopySpaceCopyContext::rebind` takes `&CopySpace` and casts it to `&'static CopySpace` using unsafe to pass to `BumpAllocator::rebind`.
- `BumpAllocator::rebind` requires `&'static dyn Space`.
- `CopySpaceCopyContext::rebind` is called in `SemiSpace::prepare_worker` and `GenCopy::prepare_worker`.
- In both cases, they pass `self.tospace()`, where `self` is `&self`.
- `prepare_worker` is called in `src/scheduler/gc_work.rs` as `mmtk.get_plan().prepare_worker(worker)`.
- `mmtk` is `&'static MMTK`, so `mmtk.get_plan()` returns `&'static dyn Plan`.
- Thus, the receiver of `prepare_worker` is actually `'static`.
- If we change `prepare_worker` in `Plan` trait to take `&'static self`, we can propagate the `'static` lifetime to `tospace()` and remove the unsafe in `rebind`.

## Attempted Changes
- None yet.

## Blockers / Insights for Next Step
- Need to update `Plan` trait and all implementations of `prepare_worker`.
