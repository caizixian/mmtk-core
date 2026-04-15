# Step Analysis

## Target
- Files: `src/mmtk.rs`, `src/scheduler/gc_work.rs`, `src/scheduler/scheduler.rs`, `src/memory_manager.rs`, `src/scheduler/mod.rs`
- Strategy: Implement `ExclusivePlanAccessProof` token to safe-guard `MMTK::get_plan_mut`.

## Findings
- `MMTK::get_plan_mut` is currently `unsafe` because it accesses `UnsafeCell<Box<dyn Plan>>` with `&self`.
- There are only 4 call sites outside `mmtk.rs`:
  1. `src/memory_manager.rs:94` — in `set_vm_space` which takes `&mut MMTK`. Can be replaced with safe `mmtk.plan.get_mut()`.
  2. `src/scheduler/gc_work.rs:59` — in `Prepare::do_work`.
  3. `src/scheduler/gc_work.rs:139` — in `Release::do_work`.
  4. `src/scheduler/scheduler.rs:566` — in `on_gc_finished`.
- The latter three are all in the scheduler or work packets executed by the scheduler during global phases where exclusive access is guaranteed.
- We can introduce a zero-sized token `ExclusivePlanAccessProof` that can only be constructed within the `scheduler` module.
- `schedule_common_work` can create the token and pass it to `Prepare` and `Release` work packets.
- `on_gc_finished` can create the token locally.
- `get_plan_mut` will be changed to take `&ExclusivePlanAccessProof` and become safe.

## Attempted Changes
- Added `ExclusivePlanAccessProof` to `src/scheduler/mod.rs`.
- Modified `MMTK::get_plan_mut` in `src/mmtk.rs` to take `&ExclusivePlanAccessProof` and removed `unsafe`.
- Updated `Prepare` and `Release` in `src/scheduler/gc_work.rs` to hold the token and use it.
- Updated `schedule_common_work` and `on_gc_finished` in `src/scheduler/scheduler.rs` to create and pass the token.
- Updated `set_vm_space` in `src/memory_manager.rs` to use `mmtk.plan.get_mut()`.
- Fixed compilation errors in `src/plan/compressor/global.rs`, `src/plan/concurrent/immix/global.rs`, and `src/plan/markcompact/global.rs` by passing the token to `Prepare::new` and `Release::new`.

## Blockers / Insights for Next Step
- Need to run `cargo check` to verify changes.
