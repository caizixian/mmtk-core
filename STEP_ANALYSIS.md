# Step Analysis (auto-saved)

## Target
- File: `src/util/rust_util/mod.rs`, `src/mmtk.rs`, and `src/policy/sft_map.rs`
- Strategy: Replace `InitializeOnce` with `OnceLock` implementation and add `Sync` bound to `SFT_MAP`.

## Findings
- `InitializeOnce` was replaced with a `OnceLock` based implementation.
- `create_sft_map` was updated to return `Box<dyn SFTMap + Sync>`.
- `SFT_MAP` was updated to use `Box<dyn SFTMap + Sync>`.
- This removes 3 unsafe blocks and 1 unsafe impl in `InitializeOnce`.
- Only 1 unsafe block remains in `InitializeOnce::get_mut`.

## Attempted Changes
- Modified `src/policy/sft_map.rs` to update `create_sft_map` return type.
- Modified `src/util/rust_util/mod.rs` to replace `InitializeOnce` implementation and remove tests.
- Modified `src/mmtk.rs` to update `SFT_MAP` type.

## Blockers / Insights for Next Step
- Need to run `cargo check` to verify compilation.
- Need to run `cargo test` to verify behavior.
