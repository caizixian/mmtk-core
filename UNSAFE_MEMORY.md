# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 331 | Current: 180 | Δ: -151
- Phase: 2

## Codebase Invariants (PROTECTED — do not prune)
- `SFT_MAP` is a global static `InitializeOnce` container. Accessing it mutably during plan initialization requires `unsafe` to bypass borrow checker.
- `GCWork` trait requires `'static` references for work packets, leading to lifetime extension unsafe blocks in space `prepare`/`release` methods.
- `Address::from_usize` is a safe `const fn` now. Unsafe blocks wrapping only this call are redundant.

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🔴 HIGH: `src/util/metadata/metadata_val_traits.rs:84-139` — Refactor `MetadataValue` trait to use `MetadataCursor` instead of `Address` to make methods safe — expected Δ: 20

## Patterns Discovered
- Redundant `unsafe` blocks wrapping safe functions like `Address::from_usize`.
- Using `MockObject` in tests to encapsulate unsafe `load`/`store` calls on `HeaderMetadataSpec`.
- **Tightened bounds**: Added `T: Sync` bound to `InitializeOnce` to ensure soundness when shared across threads.
- Replaced unsafe raw pointer dereference in bpftrace workaround with safe `str::as_bytes().first()`.
- **Safe Abstraction**: Replaced `'static` plan reference in `GCTrigger` with `Weak<dyn Plan>` and used `Arc<dyn Plan>` in `MMTK` to avoid unsafe lifetime extension.
- **Refactoring**: Removed raw pointer `worker: *mut GCWorker` from `ConcurrentTraceObjects` and replaced it with safe alternatives (storing `mmtk` reference and `tls` data), making it auto-derived `Send` and eliminating unsafe block.
- **API Cleanup**: Removed `from_raw_address_unchecked` as it was unused in core and replaced its usage in `dummyvm` with safe `from_raw_address().unwrap()`.
- **Safe Abstraction**: Used `MetadataCursor` to encapsulate unsafe loads and stores in `SideMetadataSpec`, allowing removal of `unsafe` from several function signatures.
- **Refactoring**: Made `find_prev_non_zero_value` safe by using atomic loads in its implementation and helpers, removing `unsafe` from signature and 1 unsafe block at call site.
- **Refactoring**: Made `SFTMap::update` and `eager_initialize` safe by taking references instead of raw pointers, removing 6 unsafe blocks at call sites.
- **Refactoring**: Used `MetadataCursor` in `src/policy/marksweepspace/native_ms/block.rs` to remove unsafe loads and stores of free cell links.
- **Refactoring**: Removed redundant unsafe blocks in `global.rs` tests wrapping safe `load` and `store` calls.
- **Safe Abstraction**: Made `VMMap::allocate_contiguous_chunks` and `free_contiguous_chunks` safe in the trait and implementations as they use internal locking.
- **Refactoring**: Eliminated unsafe blocks in `header_metadata.rs` tests by using `MetadataCursor` directly in `MockObject` to replicate non-atomic loads and stores.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/scheduler/worker.rs` — Remaining unsafe are trait impls for Send/Sync [Phase 2 confirmed].
- `src/policy/marksweepspace/native_ms/global.rs` — Irreducible lifetime extension for `GCWork` packets [Phase 2 confirmed].
- `src/util/metadata/side_metadata/sanity.rs` — Fixed redundant unsafe block, remaining are irreducible or valid assertions [Phase 2 confirmed].
- `src/util/metadata/side_metadata/global.rs` — Production unsafe in `load`/`store` is irreducible due to concurrent access invariants; tests were cleaned up [Phase 2 confirmed].
- `src/util/metadata/side_metadata/helpers.rs` — Implementation of `MetadataCursor` abstraction, irreducible without moving unsafe to call sites [Phase 2 confirmed].
- `src/util/metadata/side_metadata/side_metadata_tests.rs` — Irreducible raw pointer manipulations for test verification [Phase 1 analysis].
- `src/util/metadata/side_metadata/ranges.rs` — Irreducible raw pointer manipulations for bit range operations [Phase 1 analysis].
- `src/policy/sft_map.rs` — Irreducible transmute for atomic fat pointers in `SFTRefStorage` [Phase 2 confirmed].
- `src/vm/slot.rs` — Irreducible raw pointer dereferences in `SimpleSlot` and `Address` impls [Phase 2 confirmed].
- `src/util/metadata/metadata_val_traits.rs` — Trait methods require unsafe for raw pointer dereferencing in atomic operations [Phase 2 confirmed].
- `src/util/metadata/pin_bit.rs` — Fixed unsafe block by using load_atomic. Remaining code is safe [Phase 1 analysis].
- `src/util/memory.rs` — Irreducible FFI calls to mmap/munmap/mprotect/madvise and tests [Phase 2 confirmed].
- `docs/dummyvm/src/api.rs` — Irreducible FFI boundary operations [Phase 2 confirmed].
- `src/util/malloc/malloc_ms_util.rs` — Irreducible FFI calls to malloc/free/calloc [Phase 2 confirmed].
- `src/util/metadata/header_metadata.rs` — Unsafe functions `load`/`store` are non-atomic/racy by design; unsafe blocks in tests call them [Phase 2 confirmed].
- `src/util/rust_util/atomic_box.rs` — Lock-free `OnceOptionBox` requires raw pointer manipulation [Phase 2 confirmed].
- `src/policy/marksweepspace/malloc_ms/global.rs` — Irreducible lifetime extension for `GCWork` packets [Phase 2 confirmed].
- `src/util/malloc/mod.rs` — Irreducible FFI calls to malloc/free [Phase 2 confirmed].
- `src/plan/global.rs` — Irreducible lifetime extension for `GCWork` packets [Phase 2 confirmed].
- `src/plan/concurrent/concurrent_marking_work.rs` — All unsafe removed or made safe by refactoring [Phase 2 confirmed].
- `src/util/rust_util/mod.rs` — `InitializeOnce` is irreducible to maintain zero-cost reads and `get_mut(&self)` [Phase 2 confirmed].
- `src/mmtk.rs` — `ProofCell::get_ref` in `get_plan` is irreducible without threading proof tokens [Phase 2 confirmed].
- `src/policy/immix/line.rs` — All unsafe blocks removed after making SideMetadataSpec methods safe [Phase 2 confirmed].
- `src/util/heap/chunk_map.rs` — All unsafe blocks removed after making SideMetadataSpec methods safe [Phase 2 confirmed].
- `src/util/address.rs` — Irreducible primitive pointer operations (`load`, `store`, `as_ref`, etc.) [Phase 2 confirmed].
- `src/util/raw_memory_freelist.rs` — Remaining unsafe is FFI call to `munmap` in `Drop` [Phase 2 confirmed].
- `src/policy/copyspace.rs` — Irreducible FFI calls to `mprotect` and lifetime extension for `BumpAllocator` [Phase 2 confirmed].
- `src/policy/marksweepspace/native_ms/block.rs` — All unsafe blocks removed by passing BlockList reference [Phase 2 confirmed].
- `src/util/alloc/allocators.rs` — Irreducible `MaybeUninit` usage for FFI layout compatibility [Phase 2 confirmed].
- `src/scheduler/affinity.rs` — Irreducible FFI calls for thread affinity [Phase 2 confirmed].
- `src/util/alloc/allocator.rs` — Irreducible unsafe impl Sync for AllocationOptionsHolder and raw heap access in `fill_alignment_gap` [Phase 2 confirmed].
- `src/policy/immix/immixspace.rs` — Only unsafe impl Sync, no unsafe blocks [Phase 1 analysis].
- `src/scheduler/gc_work.rs` — Irreducible raw pointer `worker: *mut GCWorker` in `ProcessEdgesBase` due to VM callback constraints and performance [Phase 2 confirmed].

## Abstraction Proposals (for Phase 2)
### MetadataCursor for side_metadata
- Target files: `src/util/metadata/side_metadata/global.rs`, `src/util/metadata/side_metadata/sanity.rs`
- Expected Δ: N/A (already implemented, but centralizes unsafe)
- Design sketch: `struct MetadataCursor(Address);` provides safe methods for load/store.
- Status: done

### MetadataCursor for MetadataValue trait
- Target files: `src/util/metadata/metadata_val_traits.rs`
- Expected Δ: 20
- Design sketch: Change `MetadataValue` trait methods to take `MetadataCursor` instead of `Address`, allowing them to be safe.
- Status: proposed
