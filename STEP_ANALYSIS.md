# Step Analysis (auto-saved)

## Target
- Holistic Review under Strategy Escalation
- Strategy: Verify all remaining unsafe locations and confirm steady state.

## Findings
- `src/mmtk.rs`: `StwProtected` uses `UnsafeCell` for performance. `Sync` impl and pointer derefs are necessary and justified by `StwProof` invariant.
- `src/util/raw_memory_freelist.rs`: `from_raw_parts_mut` is centralized in `grow_list_by_blocks` to create a safe slice view. Removing it would increase unsafe blocks at access sites.
- `src/policy/sft_map.rs`: Lifetime extension is justified by spaces living forever. `SFTRefStorage::load` is lock-free and needs raw pointer deref.
- `src/vm/slot.rs`: `SimpleSlot::as_atomic` and `MemorySlice::copy` are low-level operations that are irreducible.
- `src/util/metadata/global.rs`: `load` and `store` are non-atomic and inherently unsafe.
- `src/util/malloc/malloc_ms_util.rs`: FFI and raw pointer manipulation in allocator are irreducible.
- `src/util/malloc/mod.rs`: FFI wrappers around C malloc are irreducible.
- `src/util/metadata/side_metadata/global.rs`: `MetadataSlot` helpers and raw memory copy are irreducible.

## Attempted Changes
- None (holistic review confirmed irreducible state).

## Blockers / Insights for Next Step
- All remaining unsafe blocks are justified by performance, FFI, or core memory management invariants. The codebase is in a steady state for Phase 3.
