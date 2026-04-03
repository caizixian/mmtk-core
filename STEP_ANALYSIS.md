# Step Analysis (auto-saved)

## Target
- File: `src/util/metadata/header_metadata.rs`
- Strategy: Remove `unsafe` from `HeaderMetadataSpec::load` and `store` by using safe atomic operations with `Relaxed` ordering. Also remove `unsafe` blocks in tests.

## Findings
- Line 130: `unsafe fn load` — can be made safe by calling `load_inner` with `Some(Ordering::Relaxed)`.
- Line 186: `unsafe fn store` — can be made safe by calling `store_inner` with `Some(Ordering::Relaxed)`.

## Attempted Changes
- Removed `unsafe` from `load` and `store` signatures in `HeaderMetadataSpec`.
- Updated them to use `Ordering::Relaxed` instead of `None`.
- Removed `unsafe` blocks from `safe_load` and `safe_store` in tests.

## Blockers / Insights for Next Step
- Need to run `cargo check` and `cargo test` to verify.
