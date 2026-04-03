# Step Analysis (auto-saved)

## Target
- File: `src/policy/sft_map.rs`
- Strategy: Use thin pointers to `SFTWrapper` containing raw fat pointer to eliminate fat pointer transmutes.

## Findings
- Previous attempt failed with 4 errors:
  1. `*const ()` is not `Send` -> Fixed by using `usize` (cast from pointer) as key in `HashMap`.
  2. `dyn SFT` is not `Sync` in `SFTWrapper` -> Fixed by using `*const (dyn SFT + Sync)` and implementing `unsafe impl Send` and `unsafe impl Sync` for `SFTWrapper`.
  3. `is_lock_free` not found for `AtomicPtr` -> Fixed by removing the assertion.
  4. Lifetime issue with `sft` reference -> Fixed by storing raw pointer `*const (dyn SFT + Sync)` instead of reference.

## Attempted Changes
- Refactor `SFTWrapper` to hold raw pointer and implement `Send`/`Sync`.
- Update `get_sft_wrapper` to use `usize` keys.
- Update `SFTRefStorage` to use `AtomicPtr` and remove transmutes.

## Blockers / Insights for Next Step
- The net reduction in unsafe blocks might be offset by 2 unsafe impls, but it eliminates fat pointer transmutes and potential locking in `AtomicDoubleWord`.
