# Step Analysis (auto-saved)

## Target
- File: All files with remaining unsafe listed in the harness.
- Strategy: Verified that all remaining unsafe blocks are irreducible or well-encapsulated.

## Findings
- `src/util/address.rs`: Core address operations are inherently unsafe and cannot be made safe without overhead.
- `src/util/memory.rs`: Wrappers around libc calls are necessary for memory management and are well-encapsulated.
- `src/util/malloc/malloc_ms_util.rs`: Raw pointer manipulation in the allocator is irreducible.
- `src/util/metadata/side_metadata/global.rs`: `MetadataSlot` helpers centralize unsafe operations.
- `src/util/malloc/mod.rs`: FFI wrappers are necessary.
- `src/mmtk.rs`: `StwProtected` uses `UnsafeCell` for performance.
- `src/util/metadata/global.rs`: Unsafe loads/stores are necessary for non-atomic access.
- `src/policy/sft_map.rs`: Lifetime extension is justified by spaces living forever.
- `src/vm/slot.rs`: `SimpleSlot::as_atomic` and `MemorySlice::copy` are well-encapsulated.
- `src/util/raw_memory_freelist.rs`: `from_raw_parts_mut` is necessary for slice view.
- `src/util/alloc/allocator.rs`: Raw memory fill in gap is irreducible.
- `src/util/heap/layout/mmapper/csm/mod.rs`: FFI call to `dzmmap` is necessary.
- `src/util/test_util/mock_vm.rs`: Lifetime hacks for mocking are irreducible.
- `src/policy/marksweepspace/native_ms/block.rs`: Raw memory write is irreducible.
- `src/policy/marksweepspace/malloc_ms/global.rs`: Passing space reference is necessary.

All files are listed in "Files NOT to Revisit" in `UNSAFE_MEMORY.md`.

## Attempted Changes
- None. Confirmed that all remaining unsafe is irreducible.

## Blockers / Insights for Next Step
- The project is in a steady state for Phase 3. No further reductions are possible without violating safety principles or creating unsound wrappers.
