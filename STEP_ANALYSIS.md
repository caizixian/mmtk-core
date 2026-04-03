# Step Analysis (auto-saved)

## Target
- File: `src/util/metadata/side_metadata/global.rs`
- Strategy: Investigate line 529 (`std::ptr::copy`) and re-evaluate irreducible files.

## Findings
- Line 529: `std::ptr::copy` is used to copy memory. Using safe slices would require `std::slice::from_raw_parts` which is unsafe, so it doesn't reduce the unsafe count or improve safety significantly.
- Many files marked as irreducible are indeed irreducible FFI boundaries or primitives (e.g., `src/util/memory.rs`, `src/util/address.rs`).

## Attempted Changes
- None yet. Analyzed the Work Queue item and verified that it is irreducible.

## Blockers / Insights for Next Step
- All remaining unsafe locations seem to be marked as irreducible in `UNSAFE_MEMORY.md`.
- Proposed a new investigation: Use `StwProof` or a similar capability token to remove `unsafe` from non-atomic `load`/`store` methods in `SideMetadataSpec` (lines 622, 654).
