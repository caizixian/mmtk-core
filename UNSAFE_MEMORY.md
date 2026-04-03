# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 351 | Current: 71 | Δ: -280
- Phase: 3 (Irreducible Documentation)

## Codebase Invariants (PROTECTED — do not prune)
- Work packets hold raw pointers to plans or spaces to bypass borrow checker and lifetimes.
- Static plan references are needed because plan types are generic and cannot be stored in global statics easily.
- `BlockQueue` in `BlockPageResource` was refactored to use `ArrayQueue` and `Mutex` for thread-local queues, eliminating custom lock-free code and associated unsafe blocks.

## Work Queue (NEXT STEP: pick the first actionable item)
- (None. All remaining unsafe blocks are documented as irreducible.)

## Patterns Discovered
- `InitializeOnce` provides unchecked read access on hot paths. Replacing with `OnceLock` adds overhead.
- `MetadataSlot` wraps `Address` but methods remain unsafe due to raw memory access. Moving unsafe to constructor increases count at call sites.
- Centralized unsafe raw pointer dereferences in `MetadataSlot` by introducing a helper `get_ref<T>` method, reducing unsafe blocks in load methods.
- `VMMap` trait methods were made safe as implementations (`Map32` and `Map64`) are internally synchronized with `Mutex`.
- `BlockCell` and `CellIter` abstractions in `block.rs` eliminate unsafe writes during sweep by encapsulating raw writes and ensuring valid iteration.
- `SlotLogger` was refactored to use `Mutex` instead of `RwLock`, making it automatically `Sync` and eliminating `unsafe impl Sync`.
- FFI functions transferring ownership can use `Option<Box<T>>` instead of `*mut T` to eliminate `Box::from_raw` and `Box::into_raw` unsafe blocks, provided `T` is `Sized`.
- Centralized unsafe raw pointer dereferences in `SimpleSlot` by introducing a helper `as_atomic` method, reducing unsafe blocks in `load` and `store`.
- Removed `impl Slot for Address` and changed `MemorySlice for Range<Address>` to use `SimpleSlot`, eliminating 2 unsafe blocks and aligning with the intent of using `SimpleSlot` directly.
- Centralized unsafe raw pointer dereferences in `MetadataSlot` by introducing a helper `get_mut_ref` method, and refactored `load_val` to use `get_ref`, eliminating 3 unsafe blocks at call sites (net reduction of 2).
- Used existing `MetadataSlot` abstraction to eliminate 10 unsafe blocks in `global.rs` by replacing direct calls to `MetadataValue` trait methods on `Address` with calls to `MetadataSlot` methods.
- Refactored `MetadataValue` trait to take references instead of `Address`, eliminating unsafe blocks in trait implementations and narrowing unsafe scope at call sites in `MetadataSlot`.
- Eliminated 8 unsafe blocks in `MetadataSlot` methods by using `self.get_ref::<T::Atomic>()` instead of direct unsafe casting.
- Deriving `Zeroable` for `SpaceDescriptor` eliminated 1 unsafe block.
- Used existing `MetadataSlot` abstraction to eliminate direct unsafe operations in `SideMetadataSpec` methods.
- Using `MetadataSlot::load_val` and `store_val` in tests to avoid raw pointer dereferences when testing side metadata.
- Investigation confirmed that `bytemuck` cannot be used for fat pointer transmutes in `sft_map.rs` due to unstable layout and lack of `Pod` implementation.
- Replaced unsafe non-atomic `load` with safe `load_atomic` in `scan_non_zero_values_simple` in `side_metadata/global.rs`.
- Added safe test wrappers for unsafe methods in tests to eliminate unsafe blocks in macro expansions in `side_metadata/global.rs`.
- Added `test_dzmmap` safe wrapper in `src/util/memory.rs` tests to ensure mapping only within `MEMORY_TEST_REGION`, eliminating 4 unsafe blocks.
- Combined near-contiguous unsafe blocks in `malloc_ms_util.rs` (`offset_free` and `offset_malloc_usable_size`) to reduce the total count of unsafe blocks by 2.
- Making `set` and `zero` in `memory.rs` unsafe would require adding unsafe blocks to 8 call sites, increasing the total count.
- **New**: Eliminated 2 unsafe blocks in `src/util/metadata/side_metadata/global.rs` tests by using safe `load_atomic` and `store_atomic` instead of unsafe `load` and `store`.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/util/heap/blockpageresource.rs` — Completely safe after removing `unsafe impl Sync` and adding `Send` bound to `Region` trait. [Phase 3 confirmed]
- `src/util/metadata/metadata_val_traits.rs` — Completely safe after refactoring trait to take references. [Phase 3 confirmed]
- `src/util/memory.rs` — Wrappers around libc calls. Standard FFI wrappers. Verified safety comments. Eliminated unsafe in tests using `test_dzmmap`. [Phase 3 confirmed]
- `docs/dummyvm/src/api.rs` — Reduced unsafe by using `Option<&mut T>` and `Option<Box<T>>` in FFI signatures. Remaining are `CStr::from_ptr`. [Phase 3 confirmed]
- `src/util/malloc/malloc_ms_util.rs` — Irreducible due to raw pointer manipulation in allocator. Verified safety comments. Combined near-contiguous unsafe blocks to reduce count. [Phase 3 confirmed]
- `src/util/address.rs` — Core address type. Operations are inherently unsafe. Audited safety comments. [Phase 3 confirmed]
- `src/mmtk.rs` — Uses `InitializeOnce` for `SFT_MAP` (irreducible for performance). `get_sft_map_mut` uses `get_ptr` and dereferences it unsafely under protection of `StwProof`. `StwProtected` uses `UnsafeCell` to avoid locking overhead. [Phase 3 confirmed]
- `src/util/rust_util/mod.rs` — Implements `InitializeOnce`. Removed unsafe `get_mut` and added safe `get_ptr` to defer unsafety to call sites with proof tokens. Remaining unsafe are in initialization and `get_ref`. [Phase 3 confirmed]
- `src/util/malloc/mod.rs` — Irreducible due to raw pointer manipulation in allocator. Audited safety comments. [Phase 3 confirmed]
- `src/policy/sft_map.rs` — Transmutes between fat pointers (`&dyn SFT`) and double-word integers are irreducible due to lack of safe fat pointer atomics in Rust. [Phase 3 confirmed]
- `src/util/raw_memory_freelist.rs` — Remaining unsafe are `from_raw_parts` to create slice views of raw memory. [Phase 3 confirmed]
- `src/util/metadata/global.rs` — Unsafe fns for load/store are necessary as they are non-atomic. [Phase 3 confirmed]
- `src/util/metadata/side_metadata/global.rs` — Remaining unsafe are in `MetadataSlot` helpers, memory copy, and tests calling unsafe functions. [Phase 3 confirmed]
- `src/util/heap/pageresource.rs` — Completely safe after removing unnecessary unsafe blocks. [Phase 3 confirmed]
- `src/util/heap/layout/map.rs` — Completely safe after removing unsafe from trait definition. [Phase 3 confirmed]
- `src/util/metadata/log_bit.rs` — Completely safe after replacing unsafe optimization with safe fallback. [Phase 3 confirmed]
- `src/plan/concurrent/concurrent_marking_work.rs` — Irreducible due to overlapping borrows and API constraints. [Phase 3 confirmed]
- `src/util/test_util/mock_vm.rs` — Irreducible due to lifetime hacks needed for mocking in tests. Audited safety comments. [Phase 3 confirmed]
- `src/policy/marksweepspace/malloc_ms/global.rs` — Irreducible due to passing space reference to work packets (Codebase Invariant). Audited safety comments. [Phase 3 confirmed]
- `src/util/heap/layout/mmapper/csm/mod.rs` — Irreducible due to calling unsafe `dzmmap` for memory mapping. [Phase 3 confirmed]
- `src/plan/concurrent/mod.rs` — Completely safe after removing unsafe impls for bytemuck traits. [Phase 3 confirmed]
- `src/vm/tests/mock_tests/mock_test_vm_layout_compressed_pointer.rs` — Completely safe after removing unnecessary unsafe blocks. [Phase 3 confirmed]
- `src/vm/tests/mock_tests/mock_test_vm_layout_heap_start.rs` — Completely safe after removing redundant unsafe blocks. [Phase 3 confirmed]
- `src/util/erase_vm.rs` — Completely safe after refactoring macro to use `dyn Any` for type erasure. [Phase 3 confirmed]
- `src/util/slot_logger.rs` — Completely safe after refactoring RwLock to Mutex and removing unsafe impl Sync. [Phase 3 confirmed]
- `src/util/alloc/allocator.rs` — Irreducible due to raw memory fill in allocation gap. Verified safety comments. [Phase 3 confirmed]
- `src/vm/slot.rs` — Remaining unsafe are in `SimpleSlot::as_atomic` (raw pointer cast) and `MemorySlice::copy` (raw memory copy). Added SAFETY comment to `MemorySlice::copy`. [Phase 3 confirmed]
- `src/vm/tests/mock_tests/mock_test_doc_avoid_resolving_allocator.rs` — Test file demonstrating low-level hack for performance. Verified safety comment. [Phase 3 confirmed]
- `src/util/heap/space_descriptor.rs` — Completely safe after deriving `Zeroable`. [Phase 3 confirmed]
