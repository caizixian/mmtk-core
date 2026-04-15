# Step Analysis (auto-saved)

## Target
- File: `src/util/metadata/side_metadata/global.rs`
- Strategy: Replace fake `'static` reference in `MetadataByteArrayRef` with `Address` and use `MetadataCursor` for safe access.

## Findings
- Line 1548: `data: &'static [u8; ENTRIES]` is a fake reference created from a raw pointer in `new`.
- Line 1577: `unsafe { &*address_to_meta_address(metadata_spec, start).to_ptr() }` creates this fake reference.
- Line 1592: `let value = self.data[index];` accesses it.
- This violates aliasing rules and is inherently unsafe. We can replace it with `Address` and use `MetadataCursor` in `get`.

## Attempted Changes
- Modified `MetadataByteArrayRef` to use `data: Address`.
- Updated `new` to assign `data: address_to_meta_address(metadata_spec, start)` without unsafe.
- Updated `get` to use `super::helpers::MetadataCursor(self.data + index).load::<u8>()`.

## Blockers / Insights for Next Step
- Need to run `cargo check` to verify.
