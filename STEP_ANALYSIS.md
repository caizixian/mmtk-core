# Step Analysis (auto-saved)

## Target
- Holistic review of remaining unsafe locations under strategy escalation.

## Findings
- `src/util/malloc/mod.rs`: The 4 unsafe blocks are thin wrappers around C `malloc`, `calloc`, `realloc`, and `free`. They are already isolated in safe functions and properly documented. Irreducible without replacing the allocator.
- `src/vm/slot.rs`: The unsafe block in `SimpleSlot::as_atomic` is a raw pointer cast necessary for atomic operations on the slot. The unsafe block in `MemorySlice::copy` is a raw memory copy. Both are fundamental and irreducible for count reduction without introducing overhead or unsafe at call sites.
- `src/util/metadata/side_metadata/global.rs`: Unsafe blocks are in `MetadataSlot` helpers (dereferencing address) and `bcopy_metadata_contiguous` (raw memory copy). Centralized in helpers or necessary for performance.
- `src/util/metadata/global.rs`: `load` and `store` are unsafe fns that delegate to other unsafe fns. Irreducible as they are non-atomic and require caller to ensure safety.
- `src/policy/sft_map.rs`: Lifetime extension in `get_sft_wrapper` and raw pointer deref in `SFTRefStorage::load` are necessary for lock-free access and justified by spaces living forever.

## Attempted Changes
- None. Confirmed that remaining unsafe blocks are irreducible or locally optimal.

## Blockers / Insights for Next Step
- The codebase is in Phase 3 (Steady State). All remaining unsafe blocks are irreducible or well-encapsulated. No further reductions are possible without violating performance constraints or safety principles.
