# Step Analysis (auto-saved)

## Target
- File: Multiple files (`sft_map.rs`, `slot.rs`, `global.rs`, `allocator.rs`, `block.rs`)
- Strategy: Verified remaining unsafe locations and looked for cross-cutting abstractions.

## Findings
- `src/policy/sft_map.rs`: Lifetime extension in `get_sft_wrapper` and raw pointer dereference in `SFTRefStorage::load` are irreducible as they are fundamental to the lock-free SFT table design and assume spaces live forever.
- `src/vm/slot.rs`: `SimpleSlot::as_atomic` and `MemorySlice::copy` are irreducible as they deal with raw memory access and bulk copy where validity cannot be proven by the compiler without overhead.
- `src/util/metadata/global.rs`: `load` and `store` are `unsafe fn` because they are non-atomic and not thread-safe.
- `src/util/metadata/side_metadata/global.rs`: `MetadataSlot` helpers centralize unsafe raw pointer dereferences. `bcopy_metadata_contiguous` uses `std::ptr::copy` for bulk copy. These are irreducible.
- `src/util/alloc/allocator.rs`: `fill_alignment_gap` uses `std::ptr::write_bytes` to fill raw memory, which is irreducible.
- `src/policy/marksweepspace/native_ms/block.rs`: `BlockCell::store_link` encapsulates a raw memory write, which is irreducible.

## Attempted Changes
- None. Analyzed files and confirmed that remaining unsafe is irreducible or well-encapsulated.

## Blockers / Insights for Next Step
- All remaining unsafe blocks listed in the harness are confirmed to be irreducible or well-encapsulated as documented in `UNSAFE_MEMORY.md`. The project is in a steady state for Phase 3 (Documentation).
