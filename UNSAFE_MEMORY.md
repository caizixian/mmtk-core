# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 331 | Current: 94 | Δ: -237
- Phase: 3
- Note: Reverted `MmapRegion` abstraction in `src/util/memory.rs` as it did not reduce unsafe code and increased count by 5. The count is back to 97.

## Codebase Invariants (PROTECTED — do not prune)
- Delayed initialization of `SFT_MAP` to `create_plan` allows populating it safely before making it globally visible, eliminating the need for `unsafe` access to it.
- `GCWork` trait requires `'static` references for work packets. We can avoid storing `'static` references by fetching the plan/space from `mmtk: &'static MMTK` in `do_work`.
- `Address::from_usize` is a safe `const fn` now. Unsafe blocks wrapping only this call are redundant.
- `SimpleSlot` uses `Address` instead of raw pointers, avoiding `unsafe impl Send`.
- `ProofCell` is used for `MMTK.plan` to allow zero-cost reads on non-hot but frequent paths, avoiding threading proof tokens.
- `InitializeOnce` is used for `SFT_MAP` to allow zero-cost reads on extreme hot paths (object tracing).

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🟢 LOW: Audit other files in `src/util/` to see if they are clean but not listed. (Audited conversions.rs, api_util.rs, constants.rs, finalizable_processor.rs, freelist.rs, is_mmtk_object.rs, epilogue.rs, int_array_freelist.rs, object_forwarding.rs, object_enum.rs, opaque_pointer.rs, logger.rs, mod.rs, options.rs, treadmill.rs).

## Patterns Discovered
- **Safe Abstraction**: Used `SFTHeader` wrapper to avoid `transmute` on fat pointers in `SFTRefStorage`, removing 3 unsafe blocks (and adding 1 unsafe impl Sync).
- Removed redundant `unsafe impl Send` and `Sync` for `MMTK` as all its fields are automatically `Send` and `Sync`.
- Replaced `unsafe impl Sync for GCWorkScheduler` by making `BucketOpenCondition` `Sync`, removing 1 unsafe impl.
- Replaced `unsafe impl Zeroable for SpaceDescriptor` with `#[derive(Zeroable)]`, removing 1 unsafe impl.
- Verified that most files in `src/util/heap` are clean of unsafe blocks.
- Refactoring: Removed raw pointer cast in `MallocSpace::release` by passing a function pointer to `MSSweepChunk` to fetch the space from `MMTK`.
- Documented safety invariants for FFI calls in tests in `src/util/memory.rs`.
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
- **Refactoring**: Refactored `MetadataValue` trait to use `MetadataCursor` instead of `Address`, removing `unsafe` from trait methods and implementations, and removing `unsafe` blocks in `helpers.rs` (eliminated ~30 unsafe items).
- **Safe Abstraction**: Used function pointers `fn(&'static MMTK<VM>) -> &Space` to allow work packets to fetch space references from MMTK without storing `'static` references, eliminating lifetime extension unsafe blocks in `native_ms/global.rs`.
- **Refactoring**: Removed redundant `unsafe impl Sync` for `SlotLogger` and `ImmixSpace` as they are automatically `Sync`.
- **Refactoring**: Removed redundant `unsafe impl Send` for `UpdateReferences` as it is automatically `Send`.
- **Refactoring**: Removed redundant `unsafe impl Send` for `WorkerLocalStat` by using `PhantomData<fn() -> C>` to allow auto-deriving `Send`.
- **Refactoring**: Replaced unsafe raw pointer dereference in `side_metadata/global.rs` tests with safe `MetadataValue::store` using `MetadataCursor`.
- **API Cleanup**: Refactored `notify_space_creation` to take a reference instead of a raw pointer, removing 1 unsafe block in `sft_map.rs`.
- **Refactoring**: Removed `impl Slot for Address` and updated `Range<Address>` to yield `SimpleSlot`, removing 2 unsafe blocks and moving towards recommended practice.
- **Refactoring**: Removed redundant `unsafe impl Send` and `Sync` for `FreeListPageResource` by adding `Send` bound to `FreeList` trait and updating `CreateFreeListResult` to use `Box<dyn FreeList + Send>`.
- **Refactoring**: Tightened lifetime bounds in `Plan::prepare_worker` to take `&'static self`, eliminating unsafe lifetime extension in `CopySpace::rebind`.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/util/metadata/vo_bit/mod.rs` — Clean: 0 unsafe blocks after removing dead code and using Relaxed atomic load [Phase 3 confirmed].
- `src/util/linear_scan.rs` — Clean: 0 unsafe blocks after making VO bit check safe [Phase 3 confirmed].
- `src/plan/concurrent/mod.rs` — Irreducible manual unsafe impls for bytemuck traits to use niche [Phase 2 confirmed].
- `src/scheduler/worker.rs` — Removed `unsafe impl Sync for WorkerGroup`. Remaining unsafe are trait impls for Send/Sync [Phase 2 confirmed].
- `src/util/metadata/side_metadata/sanity.rs` — Fixed redundant unsafe block, remaining are irreducible or valid assertions [Phase 2 confirmed].
- `src/util/metadata/side_metadata/global.rs` — Production unsafe in `load`/`store` is irreducible due to concurrent access invariants; tests were cleaned up [Phase 2 confirmed].
- `src/util/metadata/side_metadata/helpers.rs` — Implementation of `MetadataCursor` abstraction, irreducible without moving unsafe to call sites. SAFETY comments added in Phase 3. [Phase 3 confirmed].
- `src/util/metadata/side_metadata/side_metadata_tests.rs` — Clean: 0 unsafe blocks [Phase 2 confirmed].
- `src/util/metadata/side_metadata/ranges.rs` — Clean: 0 unsafe blocks [Phase 2 confirmed].
- `src/policy/sft_map.rs` — Irreducible transmute for atomic fat pointers in SFTRefStorage. Clear methods made safe. [Phase 3 re-confirmed].
- `src/vm/tests/mock_tests/mock_test_vm_layout_heap_start.rs` — Irreducible manual offset arithmetic to demonstrate avoiding resolution in doc example [Phase 2 confirmed].
- `src/vm/slot.rs` — Removed `impl Slot for Address`. Remaining are irreducible raw pointer dereferences in `SimpleSlot` and raw memory copy. SAFETY comments added in Phase 3. [Phase 3 confirmed].
- `src/util/metadata/metadata_val_traits.rs` — Clean: 0 unsafe blocks after refactoring to use `with_atomic` [Phase 3 confirmed].
- `src/util/metadata/pin_bit.rs` — Fixed unsafe block by using load_atomic. Remaining code is safe [Phase 2 confirmed].
- `src/util/memory.rs` — Irreducible FFI calls to mmap/munmap/mprotect/madvise. Re-evaluated Phase 2 abstraction (MmapRegion) but reverted as it didn't reduce count. [Phase 3 confirmed].
- docs/dummyvm/src/api.rs — Refactored some FFI functions to use Option<&mut T>, removing 3 unsafe blocks. Remaining unsafe are irreducible FFI boundary operations. [Phase 3 confirmed].
- `src/util/malloc/malloc_ms_util.rs` — Irreducible FFI calls to malloc/free/calloc. Safety invariants documented in Phase 3. [Phase 2 confirmed].
- `src/util/metadata/header_metadata.rs` — Unsafe functions `load`/`store` are non-atomic/racy by design; unsafe blocks in tests call them [Phase 2 confirmed].
- `src/util/metadata/global.rs` — `load` and `store` are non-atomic and not thread-safe by design [Phase 2 confirmed].
- `src/util/rust_util/atomic_box.rs` — Lock-free `OnceOptionBox` requires raw pointer manipulation. Attempted to use derive(Zeroable) in Phase 3, but AtomicPtr is not Zeroable in bytemuck 1.14.0. Confirmed irreducible to maintain minimal space overhead in `Vec<OnceOptionBox>`. [Phase 3 confirmed].
- `src/policy/marksweepspace/malloc_ms/global.rs` — Irreducible lifetime extension for `GCWork` packets [Phase 2 confirmed].
- `src/util/malloc/mod.rs` — Irreducible FFI calls to malloc/free [Phase 2 confirmed].
- `src/plan/global.rs` — Irreducible lifetime extension for `GCWork` packets [Phase 2 confirmed].
- `src/plan/concurrent/concurrent_marking_work.rs` — All unsafe removed or made safe by refactoring [Phase 2 confirmed].
- `src/util/rust_util/mod.rs` — `InitializeOnce` is irreducible to maintain zero-cost reads for `SFT_MAP` on hot path. `ProofCell` `Sync` is irreducible. Safety comments added for missing `unsafe impl Sync` in Phase 3. [Phase 3 confirmed].
- `src/util/rust_util/zeroed_alloc.rs` — `new_zeroed_vec` requires manual zeroed allocation and `Vec::from_raw_parts` for performance; `bytemuck::zeroed_vec` is not available in version 1.14.0. Handled zero size case and added safety comments in Phase 3. [Phase 3 confirmed].
- `src/mmtk.rs` — `ProofCell::get_ref` in `get_plan` is irreducible without threading proof tokens. Re-evaluated: confirmed irreducible to maintain zero-cost reads on hot allocation paths. [Phase 3 confirmed]
- `src/policy/immix/line.rs` — All unsafe blocks removed after making SideMetadataSpec methods safe [Phase 2 confirmed].
- `src/util/heap/chunk_map.rs` — All unsafe blocks removed after making SideMetadataSpec methods safe [Phase 2 confirmed].
- `src/util/address.rs` — Irreducible primitive pointer operations (`load`, `store`, `as_ref`, etc.) [Phase 2 confirmed].
- `src/util/raw_memory_freelist.rs` — Remaining unsafe is FFI call to `munmap` in `Drop` [Phase 2 confirmed].
- `src/policy/copyspace.rs` — Irreducible FFI calls to `mprotect`. Lifetime extension for `BumpAllocator` removed by tightening bounds in `Plan::prepare_worker`. [Phase 3 confirmed].
- `src/util/alloc/allocators.rs` — Irreducible `MaybeUninit` usage for FFI layout compatibility [Phase 2 confirmed].
- `src/scheduler/affinity.rs` — Irreducible FFI calls for thread affinity [Phase 2 confirmed].
- `src/util/alloc/allocator.rs` — Irreducible raw heap access in `fill_alignment_gap`. `unsafe impl Sync` for `AllocationOptionsHolder` removed by using `Mutex`. [Phase 2 confirmed].
- `src/policy/immix/immixspace.rs` — Clean: 0 unsafe blocks, no unsafe impl Sync [Phase 2 confirmed].
- `src/scheduler/gc_work.rs` — Irreducible raw pointer `worker: *mut GCWorker` in `ProcessEdgesBase` due to VM callback constraints and performance [Phase 2 confirmed].
- `src/policy/marksweepspace/native_ms/global.rs` — All unsafe blocks removed by function pointer refactoring [Phase 2 confirmed].
- `src/plan/marksweep/global.rs` — All unsafe blocks removed by function pointer refactoring [Phase 2 confirmed].
- `src/util/test_util/fixtures.rs` — Irreducible unsafe in `get_mmtk_mut` and `Drop` due to `'static` requirement on `bind_mutator` and `Box::leak` usage [Phase 2 confirmed].
- `src/util/test_util/mock_vm.rs` — Irreducible lifetime removal in `lifetime!` macro for mocking VMBinding in tests [Phase 3 confirmed].
- `src/policy/marksweepspace/malloc_ms/metadata.rs` — Remaining unsafe is unsafe fn signature for SweepProof constructor [Phase 2 confirmed].
- `src/policy/markcompactspace.rs` — Irreducible raw heap access for forwarding pointer. Encapsulated in safe functions. [Phase 2 confirmed].
- `src/policy/marksweepspace/native_ms/block.rs` — Clean: 0 unsafe blocks [Phase 2 confirmed].
- `src/util/erase_vm.rs` — Irreducible unsafe in macro for type erasure [Phase 2 confirmed].
- `src/util/heap/accounting.rs` — Clean: 0 unsafe blocks [Phase 2 confirmed].
- `src/util/heap/externalpageresource.rs` — Clean: 0 unsafe blocks [Phase 2 confirmed].
- `src/util/heap/vmrequest.rs` — Clean: 0 unsafe blocks [Phase 2 confirmed].
- `src/util/heap/heap_meta.rs` — Clean: 0 unsafe blocks [Phase 2 confirmed].
- `src/util/heap/blockpageresource.rs` — Clean: 0 unsafe blocks [Phase 2 confirmed].
- `src/util/heap/freelistpageresource.rs` — Clean: 0 unsafe blocks [Phase 2 confirmed].
- `src/util/heap/monotonepageresource.rs` — Clean: 0 unsafe blocks [Phase 2 confirmed].
- `src/util/heap/pageresource.rs` — Clean: 0 unsafe blocks [Phase 3 confirmed].
- `src/util/heap/regionpageresource.rs` — Clean: 0 unsafe blocks [Phase 3 confirmed].
- `src/util/heap/space_descriptor.rs` — Clean: 0 unsafe blocks [Phase 3 confirmed].
- `src/policy/copy_context.rs` — Clean: 0 unsafe blocks [Phase 2 confirmed].
- `src/policy/gc_work.rs` — Clean: 0 unsafe blocks [Phase 2 confirmed].
- `src/policy/immortalspace.rs` — Clean: 0 unsafe blocks [Phase 2 confirmed].
- `src/policy/largeobjectspace.rs` — Clean: 0 unsafe blocks [Phase 2 confirmed].
- `src/policy/lockfreeimmortalspace.rs` — Clean: 0 unsafe blocks [Phase 2 confirmed].
- `src/policy/mod.rs` — Clean: 0 unsafe blocks [Phase 2 confirmed].
- `src/policy/sft.rs` — Clean: 0 unsafe blocks [Phase 2 confirmed].
- `src/policy/space.rs` — Clean: 0 unsafe blocks [Phase 2 confirmed].
- `src/policy/vmspace.rs` — Clean: 0 unsafe blocks [Phase 2 confirmed].
- `src/policy/marksweepspace/native_ms/block_list.rs` — Clean: 0 unsafe blocks [Phase 2 confirmed].
- `src/policy/compressor/compressorspace.rs` — Clean: 0 unsafe blocks [Phase 2 confirmed].
- `benches/regular_bench/bulk_meta/bscan.rs` — Clean: 0 unsafe blocks after using safe AlignedBuffer [Phase 2 confirmed].
- `benches/regular_bench/bulk_meta/bzero_bset.rs` — Clean: 0 unsafe blocks after using AlignedBuffer and slice::fill [Phase 2 confirmed].
- `src/plan/mutator_context.rs` — Clean: 0 unsafe blocks [Phase 2 confirmed].
- `src/util/alloc/bumpallocator.rs` — Clean: 0 unsafe blocks [Phase 2 confirmed].
- `src/util/conversions.rs` — Clean: 0 unsafe blocks [Phase 3 confirmed].
- `src/util/api_util.rs` — Clean: 0 unsafe blocks [Phase 3 confirmed].
- `src/util/constants.rs` — Clean: 0 unsafe blocks [Phase 3 confirmed].
- `src/util/finalizable_processor.rs` — Clean: 0 unsafe blocks [Phase 3 confirmed].
- `src/util/freelist.rs` — Clean: 0 unsafe blocks [Phase 3 confirmed].
- `src/util/is_mmtk_object.rs` — Clean: 0 unsafe blocks [Phase 3 confirmed].
- `src/util/epilogue.rs` — Clean: 0 unsafe blocks [Phase 3 confirmed].
- `src/util/int_array_freelist.rs` — Clean: 0 unsafe blocks [Phase 3 confirmed].
- `src/util/object_forwarding.rs` — Clean: 0 unsafe blocks [Phase 3 confirmed].
- `src/util/object_enum.rs` — Clean: 0 unsafe blocks [Phase 3 confirmed].
- `src/util/opaque_pointer.rs` — Clean: 0 unsafe blocks [Phase 3 confirmed].
- `src/util/logger.rs` — Clean: 0 unsafe blocks [Phase 3 confirmed].
- `src/util/mod.rs` — Clean: 0 unsafe blocks [Phase 3 confirmed].
- `src/util/options.rs` — Clean: 0 unsafe blocks [Phase 3 confirmed].
- `src/util/treadmill.rs` — Clean: 0 unsafe blocks [Phase 3 confirmed].

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
- Status: done

### MmapRegion for memory mapping
- Target files: `src/util/memory.rs`, `src/util/heap/layout/mmapper/csm/mod.rs`
- Expected Δ: TBD
- Design sketch: A type that owns a memory mapping and guarantees safety for reads and writes within its bounds.
- Status: abandoned (reverted as it did not reduce unsafe count)
