# Step Analysis (auto-saved)

## Target
- File: `src/policy/marksweepspace/native_ms/block.rs`
- Strategy: Replace non-atomic `load`/`store` on `SideMetadataSpec` with `load_atomic`/`store_atomic` with `SeqCst` ordering to remove unsafe blocks.

## Findings
- Lines 69, 73, 81, 85: These methods in `SideMetadataSpecBlockExt` use `load` and `store` which are unsafe.
- `SideMetadataSpec` has `load_atomic` and `store_atomic` which are safe.
- According to `UNSAFE_MEMORY.md` patterns, we can use `store_atomic` to replace raw stores in metadata updates.
- Line 208: `store_block_list` also uses `store`. Can be replaced with `store_atomic`.

## Attempted Changes
- Replaced non-atomic `load` and `store` with `load_atomic` and `store_atomic` using `Ordering::SeqCst` in `SideMetadataSpecBlockExt` and `store_block_list`.
- Removed 5 unsafe blocks.

## Blockers / Insights for Next Step
- Need to run `cargo check` and `cargo test` to verify.
