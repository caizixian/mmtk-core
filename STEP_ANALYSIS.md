# Step Analysis (auto-saved)

## Target
- File: `src/util/rust_util/mod.rs`
- Strategy: Replace `InitializeOnce` with `OnceLock` to reduce unsafe count.

## Findings
- Attempted to replace `InitializeOnce` with `OnceLock` in `src/util/rust_util/mod.rs`.
- This required adding `Send` bound to `SFT_MAP` in `src/mmtk.rs` and `create_sft_map` in `src/policy/sft_map.rs` to satisfy `OnceLock`'s `Sync` requirement.
- However, compilation failed with `error: casting &T to &mut T is undefined behavior` in `InitializeOnce::get_mut` because `OnceLock::get` returns an immutable reference and we need to return a mutable reference from a shared reference (`&self`). This triggers the `invalid_reference_casting` lint in Rust 1.92+.
- Reverted all changes to restore the build.

## Attempted Changes
- Modified `src/util/rust_util/mod.rs`, `src/mmtk.rs`, and `src/policy/sft_map.rs`. All changes were reverted after build failure.

## Blockers / Insights for Next Step
- `InitializeOnce` is irreducible for performance and because of the reference casting issue described above.
- All files with unsafe are currently marked as irreducible or audited in `UNSAFE_MEMORY.md`.
- Next step should proceed with auditing remaining files for missing safety comments as per the work queue.
