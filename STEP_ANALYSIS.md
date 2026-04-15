# Step Analysis (auto-saved)

## Target
- File: `src/util/metadata/header_metadata.rs`
- Strategy: Eliminate unsafe blocks in tests calling `load`/`store` by using `MetadataCursor` directly in `MockObject`.

## Findings
- Line 664 (in file view): `unsafe { spec.load::<T>(self.obj, optional_mask) }` — Eliminable by replicating non-atomic `load_inner` logic using `MetadataCursor` safe methods.
- Line 668 (in file view): `unsafe { spec.store::<T>(self.obj, val, optional_mask) }` — Eliminable by replicating non-atomic `store_inner` logic using `MetadataCursor` safe methods.

## Attempted Changes
- Plan to modify `MockObject` methods `load` and `store` to use `MetadataCursor` directly.

## Blockers / Insights for Next Step
- None so far.
