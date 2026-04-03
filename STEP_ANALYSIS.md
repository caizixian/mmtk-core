# Step Analysis (auto-saved)

## Target
- File: `src/util/metadata/side_metadata/global.rs`
- Strategy: Introduce `MetadataSlot` abstraction to encapsulate `addr.as_ref::<AtomicU8>()` and related unsafe operations.

## Findings
- Line 214: `unsafe { addr.as_ref::<AtomicU8>() }.fetch_and(...)` — can be wrapped in `MetadataSlot`.
- Line 251: `unsafe { addr.as_ref::<AtomicU8>() }.fetch_or(...)` — can be wrapped in `MetadataSlot`.
- Line 447-450: `as_ref::<AtomicU8>()` used for load and store in `bcopy_metadata_contiguous`.
- Line 592: `meta_addr.atomic_load::<AtomicU8>(order)` — can be wrapped.
- Line 618: `fetch_update` on `meta_addr` — can be wrapped.

## Attempted Changes
- Introduced `MetadataSlot` struct with `fetch_and`, `fetch_or`, `load`, `store`, `fetch_update`, and `compare_exchange` methods.
- Replaced unsafe blocks in `zero_meta_bits`, `set_meta_bits`, `bcopy_metadata_contiguous`, `load_atomic`, `compare_exchange_atomic`, and `store_atomic` with `MetadataSlot` calls.

## Blockers / Insights for Next Step
- None.
