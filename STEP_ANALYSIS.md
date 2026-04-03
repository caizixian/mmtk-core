# Step Analysis (auto-saved)

## Target
- File: src/policy/sft_map.rs and src/mmtk.rs
- Strategy: Analyze if remaining unsafe blocks can be reduced or abstracted.

## Findings
- src/policy/sft_map.rs Line 111: Irreducible. Lifetime extension for `'static` reference in global map.
- src/policy/sft_map.rs Line 138: Irreducible. Dereferencing `AtomicPtr` to get `&dyn SFT`.
- src/mmtk.rs Line 136: Irreducible. `unsafe impl Sync` for `StwProtected`.
- src/mmtk.rs Line 149, 155: Irreducible. Dereferencing `UnsafeCell` pointer.

## Attempted Changes
- None.

## Blockers / Insights for Next Step
- All remaining unsafe blocks are in files marked as "NOT to Revisit" and have been confirmed irreducible. The project is in Phase 3.
