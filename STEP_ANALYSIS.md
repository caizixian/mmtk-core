# Step Analysis (auto-saved)

## Target
- Holistic review of remaining unsafe locations under strategy escalation, focusing on `sft_map.rs`, `allocator.rs`, `mmapper/csm/mod.rs`, and `mock_vm.rs`.

## Findings
- `src/policy/sft_map.rs`: Analyzed `get_sft_wrapper` and `SFTRefStorage::load`. Proposed changing `SFTWrapper` to hold a raw pointer instead of a reference. This would eliminate the unsafe block in `get_sft_wrapper` (lifetime extension), but would require adding `unsafe impl Sync for SFTWrapper` to allow sharing references, resulting in a neutral delta in unsafe count.
- `src/util/alloc/allocator.rs`: The unsafe block in `fill_alignment_gap` uses `std::ptr::write_bytes`. Replacing it with slice operations would still require unsafe to create the slice, yielding no reduction.
- `src/util/heap/layout/mmapper/csm/mod.rs`: The unsafe block calls `dzmmap` for quarantined chunks. This is necessary because `dzmmap_noreplace` would fail on already mapped (quarantined) ranges.
- `src/util/test_util/mock_vm.rs`: The `lifetime!` macro uses `transmute` to remove lifetimes for mocking purposes. This is a test utility and the unsafety is justified by the difficulty of making `MockVM` generic over lifetimes.

## Attempted Changes
- Analyzed `sft_map.rs` for potential reduction by changing `SFTWrapper` field type, but concluded it would result in a neutral delta due to need for `unsafe impl Sync`.

## Blockers / Insights for Next Step
- Confirmed that remaining unsafe blocks are irreducible or locally optimal. The codebase remains in Phase 3 (Steady State).
