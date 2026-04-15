# Step Analysis (auto-saved)

## Target
- File: `src/util/metadata/side_metadata/global.rs`
- Strategy: Refactor `SideMetadataOffset` from a union to an enum to eliminate unsafe accesses to its fields.

## Findings
- `SideMetadataOffset` is a union of `Address` and `usize`.
- Accessing its fields requires `unsafe` blocks.
- It is used in `SideMetadataSpec` which knows which field is valid based on `uses_contiguous_side_metadata()`.
- All accesses to `offset.addr` and `offset.rel_offset` are in `src/util/metadata/side_metadata/global.rs`.
- Changing it to an enum will eliminate at least 7 unsafe blocks in this file and likely more in `PartialEq` and `Hash` implementations.

## Attempted Changes
- Refactored `SideMetadataOffset` to an enum in `src/util/metadata/side_metadata/global.rs`.
- Updated all accesses to use pattern matching instead of unsafe union field access.
- Derived `PartialEq`, `Eq`, and `Hash` for `SideMetadataOffset`.
- Updated `ZERO_OFFSET` to use `SideMetadataOffset::Rel(0)`.
- Ran `cargo check` and it succeeded!

## Blockers / Insights for Next Step
- Need to run `cargo test` to verify the refactor at runtime.
