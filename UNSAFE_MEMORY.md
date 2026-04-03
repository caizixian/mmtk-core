# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 351 | Current: 199 | Δ: -152
- Phase: 3 (Irreducible Documentation)

## Codebase Invariants (PROTECTED — do not prune)
- Work packets hold raw pointers to plans or spaces to bypass borrow checker and lifetimes.
- Static plan references are needed because plan types are generic and cannot be stored in global statics easily.
- `BlockQueue` in `BlockPageResource` was refactored to use `ArrayQueue` and `Mutex` for thread-local queues, eliminating custom lock-free code and associated unsafe blocks.

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🟢 LOW: `src/util/address.rs:228-232` — document `load` method with SAFETY comments — expected Δ: 0

## Patterns Discovered
- `InitializeOnce` provides unchecked read access on hot paths. Replacing with `OnceLock` adds overhead.
- `MetadataSlot` wraps `Address` but methods remain unsafe due to raw memory access. Moving unsafe to constructor increases count at call sites.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/util/metadata/side_metadata/global.rs` — Remaining unsafe are irreducible function signatures and raw memory copy. [Phase 2 confirmed]
- `src/util/metadata/metadata_val_traits.rs` — Defines contract for loading metadata values. Inherently unsafe. [Phase 2 confirmed]
- `src/util/memory.rs` — Wrappers around libc calls. Standard FFI wrappers. [Phase 2 confirmed]
- `docs/dummyvm/src/api.rs` — Dummy VM implementation for testing/docs. Not part of production code. [Phase 2 confirmed]
- `src/util/malloc/malloc_ms_util.rs` — Irreducible due to raw pointer manipulation in allocator. [Phase 2 confirmed]
- `src/util/address.rs` — Core address type. Operations are inherently unsafe. [Phase 2 confirmed]
- `src/mmtk.rs` — Uses `InitializeOnce` for `SFT_MAP`. Irreducible for performance. [Phase 2 confirmed]
- `src/util/rust_util/mod.rs` — Implements `InitializeOnce`. Irreducible for performance. [Phase 2 confirmed]
- `src/vm/slot.rs` — Refactored SimpleSlot to use Address instead of raw pointer. Remaining unsafe are dereferences in load/store. [Phase 3 confirmed]
- `src/util/malloc/mod.rs` — Irreducible due to raw pointer manipulation in allocator. [Phase 2 confirmed]
- `src/policy/sft_map.rs` — Transmutes are irreducible due to fat pointer provenance. [Phase 2 confirmed]
- `src/policy/marksweepspace/native_ms/block.rs` — Irreducible due to raw pointer manipulation. [Phase 2 confirmed]
- `src/util/raw_memory_freelist.rs` — Remaining unsafe are `from_raw_parts` to create slice views of raw memory. [Phase 3 confirmed]
- `src/util/metadata/global.rs` — Unsafe fns for load/store are necessary as they are non-atomic. [Phase 3 confirmed]
- `src/util/heap/pageresource.rs` — Documented irreducible unsafe calls to VMMap with SAFETY comments. [Phase 3 confirmed]
