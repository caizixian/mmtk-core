# Step Analysis (auto-saved)

## Target
- File: src/policy/marksweepspace/native_ms/global.rs, src/plan/marksweep/global.rs
- Strategy: Safe Abstraction / Refactoring using function pointers to remove lifetime extension unsafe blocks.

## Findings
- Downcasting to `MarkSweep` plan in `native_ms` fails because `marksweep` module is private.
- Moving work packets to `marksweep` module fails because `native_ms` cannot access them.
- Solution: Keep work packets in `native_ms` but pass a function pointer `fn(&'static MMTK<VM>) -> &MarkSweepSpace<VM>` to them. This function pointer is provided by the plan and is `'static`.
- This allows `native_ms` to fetch the space reference safely without knowing the concrete plan type or violating privacy.

## Attempted Changes
- Plan to update `PrepareChunkMap`, `ReleaseMarkSweepSpace`, `SweepChunk`, and `RecycleBlocks` to take a function pointer.
- Plan to update `prepare`, `release`, and `generate_sweep_tasks` to take the function pointer.
- Plan to update `MarkSweep::prepare` and `release` to pass the function pointer.
