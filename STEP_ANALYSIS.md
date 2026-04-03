# Step Analysis (auto-saved)

## Target
- File: `src/util/metadata/side_metadata/global.rs`
- Strategy: Phase 2 - Use `MetadataSlot` to eliminate unsafe operations in `load` and `store`.

## Findings
- `SideMetadataSpec::load` and `store` were using raw memory access (`meta_addr.load/store`).
- Replaced them with `self.slot_for(...).load/store()` which are safe operations.
- This eliminates unsafe operations inside the closures passed to `side_metadata_access`.

## Attempted Changes
- Updated `load` and `store` in `global.rs`.
- Created `safe_access.rs` and updated `mod.rs` and `metadata_val_traits.rs` in previous steps.

## Blockers / Insights for Next Step
- Need to check if this reduces the unsafe count or if it's just a cleanup (since the functions themselves are still `unsafe fn`).
- If it doesn't reduce count, we may need to make the functions safe by adding `StwProof` or similar.
