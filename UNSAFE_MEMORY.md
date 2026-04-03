# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 351 | Current: 175 | Δ: -176
- Phase: 3 (Irreducible Documentation)

## Codebase Invariants (PROTECTED — do not prune)
- Work packets hold raw pointers to plans or spaces to bypass borrow checker and lifetimes.
- Static plan references are needed because plan types are generic and cannot be stored in global statics easily.
- `BlockQueue` in `BlockPageResource` was refactored to use `ArrayQueue` and `Mutex` for thread-local queues, eliminating custom lock-free code and associated unsafe blocks.

## Work Queue (NEXT STEP: pick the first actionable item)
- (No actionable items. All remaining unsafe is irreducible and documented.)

## Patterns Discovered
- `InitializeOnce` provides unchecked read access on hot paths. Replacing with `OnceLock` adds overhead.
- `MetadataSlot` wraps `Address` but methods remain unsafe due to raw memory access. Moving unsafe to constructor increases count at call sites.
- Centralized unsafe raw pointer dereferences in `MetadataSlot` by introducing a helper `get_ref<T>` method, reducing unsafe blocks in load methods.
- `VMMap` trait methods were made safe as implementations (`Map32` and `Map64`) are internally synchronized with `Mutex`.
- `BlockCell` and `CellIter` abstractions in `block.rs` eliminate unsafe writes during sweep by encapsulating raw writes and ensuring valid iteration.
- **New**: `SlotLogger` was refactored to use `Mutex` instead of `RwLock`, making it automatically `Sync` and eliminating `unsafe impl Sync`.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/util/metadata/side_metadata/global.rs` — Remaining unsafe are irreducible function signatures and raw memory copy. [Phase 2 confirmed]
- `src/util/metadata/metadata_val_traits.rs` — Defines contract for loading metadata values. Inherently unsafe. [Phase 2 confirmed]
- src/util/memory.rs — Wrappers around libc calls. Standard FFI wrappers. Verified safety comments. [Phase 3 confirmed]
- `docs/dummyvm/src/api.rs` — Dummy VM implementation for testing/docs. Not part of production code. [Phase 2 confirmed]
- src/util/malloc/malloc_ms_util.rs — Irreducible due to raw pointer manipulation in allocator. Verified safety comments. [Phase 3 confirmed]
- `src/util/address.rs` — Core address type. Operations are inherently unsafe. [Phase 2 confirmed]
- `src/mmtk.rs` — Uses `InitializeOnce` for `SFT_MAP`. Irreducible for performance. [Phase 2 confirmed]
- `src/util/rust_util/mod.rs` — Implements `InitializeOnce`. Irreducible for performance. [Phase 2 confirmed]
- `src/vm/slot.rs` — Refactored SimpleSlot to use Address instead of raw pointer. Remaining unsafe are dereferences in load/store. [Phase 3 confirmed]
- `src/util/malloc/mod.rs` — Irreducible due to raw pointer manipulation in allocator. [Phase 2 confirmed]
- `src/policy/sft_map.rs` — Transmutes are irreducible due to fat pointer provenance. [Phase 2 confirmed]
- `src/policy/marksweepspace/native_ms/block.rs` — Refactored sweep to use safe iterator. Remaining unsafe is encapsulated in BlockCell::store_link. [Phase 3 confirmed]
- `src/util/raw_memory_freelist.rs` — Remaining unsafe are `from_raw_parts` to create slice views of raw memory. [Phase 3 confirmed]
- `src/util/metadata/global.rs` — Unsafe fns for load/store are necessary as they are non-atomic. [Phase 3 confirmed]
- `src/util/heap/pageresource.rs` — Completely safe after removing unnecessary unsafe blocks. [Phase 3 confirmed]
- `src/util/heap/layout/map.rs` — Completely safe after removing unsafe from trait definition. [Phase 3 confirmed]
- `src/util/metadata/log_bit.rs` — Completely safe after replacing unsafe optimization with safe fallback. [Phase 3 confirmed]
- `src/plan/concurrent/concurrent_marking_work.rs` — Irreducible due to overlapping borrows and API constraints. [Phase 3 confirmed]
- `src/util/test_util/mock_vm.rs` — Irreducible due to lifetime hacks needed for mocking in tests. [Phase 3 confirmed]
- `src/policy/marksweepspace/malloc_ms/global.rs` — Irreducible due to passing space reference to work packets (Codebase Invariant). [Phase 3 confirmed]
- `src/util/heap/layout/mmapper/csm/mod.rs` — Irreducible due to calling unsafe `dzmmap` for memory mapping. [Phase 3 confirmed]
- `src/plan/concurrent/mod.rs` — Completely safe after removing unsafe impls for bytemuck traits. [Phase 3 confirmed]
- `src/vm/tests/mock_tests/mock_test_vm_layout_compressed_pointer.rs` — Completely safe after removing unnecessary unsafe blocks. [Phase 3 confirmed]
- `src/vm/tests/mock_tests/mock_test_vm_layout_heap_start.rs` — Completely safe after removing redundant unsafe blocks. [Phase 3 confirmed]
- `src/util/erase_vm.rs` — Irreducible due to type erasure macro storing reference as usize. [Phase 3 confirmed]
- src/util/slot_logger.rs — Completely safe after refactoring RwLock to Mutex and removing unsafe impl Sync. [Phase 3 confirmed]
- src/util/alloc/allocator.rs — Irreducible due to raw memory fill in allocation gap. Verified safety comments. [Phase 3 confirmed]
