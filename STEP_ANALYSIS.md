# Step Analysis (auto-saved)

## Target
- File: `src/plan/marksweep/global.rs`, `src/policy/marksweepspace/malloc_ms/global.rs`
- Strategy: Thread `ExclusivePlanAccessProof` to `MallocSpace::release` to eliminate `unsafe { SweepProof::new_unchecked() }`.

## Findings
- `src/policy/marksweepspace/malloc_ms/global.rs:562`: `unsafe { SweepProof::new_unchecked() }` can be replaced by a safe constructor taking `&ExclusivePlanAccessProof`.
- `ExclusivePlanAccessProof` is available in `Release` work packet but not passed to `Plan::release`.

## Attempted Changes
- Added `release_with_proof` to `Plan` trait in `src/plan/global.rs`.
- Updated `Release` work packet in `src/scheduler/gc_work.rs` to call `release_with_proof`.
- Updated `MallocSpace::release` in `src/policy/marksweepspace/malloc_ms/global.rs` to take proof and use it to create `SweepProof` safely.
- Updated `MarkSweepSpace::release` in `src/policy/marksweepspace/native_ms/global.rs` to take proof (dummy).
- Overrode `release_with_proof` in `src/plan/marksweep/global.rs` to pass the proof to `ms.release`.
- Added safe constructor for `SweepProof` in `src/policy/marksweepspace/malloc_ms/metadata.rs`.

## Blockers / Insights for Next Step
- Need to run `cargo check` to verify compilation.
- If successful, tests should be run.
