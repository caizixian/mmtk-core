# Step Analysis (auto-saved)

## Target
- File: `src/util/metadata/side_metadata/global.rs`
- Strategy: Replace `&'static [u8; ENTRIES]` with `Address` (or remove it and use `load_atomic`) in `MetadataByteArrayRef` to eliminate unsafe block in `new`.

## Findings
- Line 1708: `data: unsafe { &*address_to_meta_address(metadata_spec, start).to_ptr() },` — Eliminable by removing `data` field and using `load_atomic` in `get`.
- By removing the `#[cfg(feature = "extreme_assertions")]` guards from `heap_range_start` and `spec`, we can store them unconditionally.
- We can then implement `get` using `self.spec.load_atomic::<u8>(data_addr, Ordering::Relaxed)` where `data_addr` is computed from `heap_range_start` and index.
- This eliminates the unsafe block in `new` and does not introduce unsafe in `get`.
- This reduces unsafe count by 1.

## Attempted Changes
- None yet.

## Blockers / Insights for Next Step
- None.
