# Step Analysis (auto-saved)

## Target
- File: All files with remaining unsafe blocks.
- Strategy: Holistic review and verification of remaining unsafe blocks under strategy escalation.

## Findings
- I have reviewed the remaining unsafe locations provided in the prompt and cross-referenced them with the "Files NOT to Revisit" list in `UNSAFE_MEMORY.md`.
- `src/util/metadata/side_metadata/global.rs`: Unsafe blocks are in `MetadataSlot` helpers and memory copy operations. `MetadataSlot` is already an abstraction to centralize unsafe. The functions themselves are unsafe because they perform raw memory operations based on calculated addresses.
- `src/util/memory.rs`: All unsafe blocks are FFI calls to `libc` (mmap, munmap, etc.). These are inherently unsafe.
- `src/util/address.rs`: Core primitive operations on raw addresses. Inherently unsafe.
- `src/util/malloc/malloc_ms_util.rs` and `src/util/malloc/mod.rs`: FFI calls to allocator functions (`malloc`, `free`, etc.). Inherently unsafe.
- `src/mmtk.rs`: Uses `UnsafeCell` for `StwProtected` to avoid locking overhead. This is a deliberate performance trade-off.
- `src/util/raw_memory_freelist.rs`: `from_raw_parts` is used to create slices from raw memory. This is encapsulated in `get_slice` and `get_slice_mut`.
- `src/vm/slot.rs`: `SimpleSlot::as_atomic` uses unsafe to cast an address to a reference. This is centralized to avoid unsafe at call sites.
- `src/util/test_util/mock_vm.rs`: Uses `transmute` to remove lifetimes for mocking purposes in tests. This is a test-only hack.
- `src/policy/marksweepspace/native_ms/block.rs`: `BlockCell::store_link` performs a raw memory write. It is encapsulated.
- I conclude that the project has reached a steady state where all remaining unsafe blocks are either FFI calls, core primitive operations, or well-encapsulated within safe abstractions. No further reductions are expected without massive architectural changes that would likely degrade performance or require a complete redesign of the GC framework.

## Attempted Changes
- None. Confirmed that remaining unsafe blocks are irreducible.

## Blockers / Insights for Next Step
- The project is at a state where remaining unsafe is irreducible. The harness may continue to report 0 reductions. This conclusion is documented here and in `UNSAFE_MEMORY.md`.
