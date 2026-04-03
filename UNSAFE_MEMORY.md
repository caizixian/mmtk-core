# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 493 | Current: 399 | Δ: -94
- Phase: 2

## Codebase Invariants (PROTECTED — do not prune)
- `Address::from_usize` is marked unsafe by design to warn about invalid addresses. Replacing it with `ZERO.add` is considered an anti-pattern as it is semantically identical.

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🔴 HIGH: Start Phase 3: Document remaining unsafe blocks in `src/util/rust_util/mod.rs` and `src/policy/marksweepspace/native_ms/block.rs` with `// SAFETY:` comments or justifications.
2. 🟡 MED: Identify other `MaybeUninit` usages in the codebase and apply safe abstractions if possible without performance regression.

## Patterns Discovered
- `MaybeUninit` arrays of size 1 can be replaced with `Option` and `unwrap()` to eliminate unsafe access.
- In tests, raw memory allocation with `alloc_zeroed` and `dealloc` can be replaced with safe `Vec` to eliminate unsafe blocks.
- `unsafe impl Sync` for types containing only atomics or Sync types can often be removed as the compiler can auto-derive Sync.
- `get_unchecked` on `Vec` can be replaced with standard indexing `[]` if we are okay with bounds checks (or if bounds are already checked).
- Using safe slices `&mut [T]` instead of raw pointers `*mut T` in tests allows using safe indexing and removes unsafe dereferences.
- **Refactoring unions to enums**: can eliminate unsafe field accesses if layout compatibility is not strictly required or if the overhead is acceptable.
- `NonZeroUsize::new_unchecked` can be replaced with `NonZeroUsize::new().expect()` if the value is known to be non-zero.
- **MetadataSlot**: Centralizes unsafe raw memory access in `safe_access.rs`.
- **Safe Constructor**: Adding `slot_for` to `SideMetadataSpec` and `HeaderMetadataSpec` allows safe access to `MetadataSlot` without unsafe blocks at call sites.
- **StwProof**: Token to prove world is stopped, allowing safe non-atomic access to metadata. Used to make `SideMetadataSpec::load` and `store` safe.
- **slot_from_meta_addr**: Added to `SideMetadataSpec` to return a `MetadataSlot` from a metadata address, allowing safe operations in `zero_meta_bits` and `set_meta_bits`.
- **Proof Token for Safe Functions**: Changing signatures of `unsafe fn` to take `&StwProof` can make them safe if the only safety invariant is no concurrent access.
- **StwProof in Tests**: Using `StwProof::new_for_tests()` to remove redundant unsafe blocks around `load` and `store` in tests.
- **OnceLock for late init**: Replacing `MaybeUninit` with `OnceLock` for late-initialized global or shared state (e.g., `GCTrigger::plan`).
- **Atomics for Interior Mutability**: Replacing `UnsafeCell` and manual locking/unsafe with atomic types (`AtomicUsize`, `AtomicBool`) can eliminate `mut_self` patterns and reduce unsafe blocks (applied to `Map64` and `Map32`).
- **Replacing std::ptr::copy with loop**: In `bcopy_metadata_contiguous`, replaced `std::ptr::copy` with a safe loop using `MetadataSlot`.
- **Refactoring MetadataByteArrayRef**: Changed it to hold `Address` and `&SideMetadataSpec` instead of `&'static [u8; ENTRIES]`, eliminating a dangerous pointer-to-reference cast.
- **Using MetadataSlot in Tests**: Replaced raw pointer dereferences in tests with `MetadataSlot::load` and `store` to eliminate unsafe blocks.
- **Replacing Address::zero() with Address::ZERO**: Eliminated 1 unsafe block in `free_list_allocator.rs`.
- **Address comparison in tests**: Instead of loading values from addresses yielded by an iterator in tests (which requires unsafe), compare the addresses directly with the expected addresses of valid objects.
- **Eliminating unsafe casts in constructors**: If a type uses `OnceLock` for late initialization, its setter can take `&self` instead of `&mut self`, avoiding the need to cast `Arc` to `&mut` in constructors when the object is shared but not yet fully initialized in the type system's view.
- **Removing unnecessary unsafe from functions**: Functions marked `unsafe` that contain no unsafe operations and rely on safe abstractions can be made safe (applied to `FreeListPageResource`).
- **Passing &mut BlockList to attempt_release**: Eliminates unsafe raw pointer dereference by passing the list reference from the caller instead of loading it from metadata.
- **Capability Token for Iterators**: Requiring `StwProof` in `ObjectIterator::new` when `ATOMIC_LOAD_VO_BIT` is false enforces safety at compile time and removes unsafe blocks from `next()`.
- **Replacing get_unchecked with get_checked**: In `SFTProcessEdges::trace_object`, replaced `get_unchecked` with `get_checked` on `SFT_MAP` to remove an unsafe block, as the function is marked as unused/deprecated.
- **StwProof for Header Metadata**: Applied `StwProof` to `HeaderMetadataSpec::load_stw` and `store_stw` and `ObjectModel::load_metadata` and `store_metadata` to make non-atomic header metadata access safe.
- **Passing spec to helpers functions**: Added `&SideMetadataSpec` parameter to scanning functions in `helpers.rs` to use `slot_from_meta_addr` and remove unsafe blocks.
- **Replacing MaybeUninit with Option in BlockQueue**: Eliminated unsafe `assume_init()` and safe initialization in `src/util/heap/blockpageresource.rs`.
- **Safe push_mut in BlockQueue**: Added `push_mut` taking `&mut self` to `BlockQueue` to allow safe pushing when the queue is local, removing 3 unsafe blocks.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/util/copy/mod.rs` — All unsafe removed.
- `src/policy/sft_map.rs` — Remaining unsafe are trait signatures and unavoidable transmutes for fat pointers in atomics.
- `src/util/alloc/allocators.rs` — Remaining unsafe are centralized in `get_allocator` implementations using `assume_init_ref/mut` on `MaybeUninit` arrays, guarded by a runtime bitmap check.
- `src/util/metadata/side_metadata/helpers.rs` — Remaining unsafe are raw loads in functions that scan metadata addresses directly without a spec. (Update: some removed by passing spec).
- `src/util/metadata/metadata_val_traits.rs` — `MetadataValue` trait methods are unsafe by design as they perform raw memory access.
- `src/util/metadata/side_metadata/side_metadata_tests.rs` — All unsafe are `Address::from_usize(...)` for creating test addresses.
- `src/policy/marksweepspace/native_ms/block.rs` — Remaining unsafe are `Address::from_usize`, `ObjectReference::from_raw_address_unchecked`, and raw pointer dereferencing for `BlockList`.
- `src/util/heap/layout/map32.rs` — Remaining unsafe are `mut_self` calls in `finalize_static_space_map` and `get_discontig_freelist_pr_ordinal` claimed to be safe due to single-threaded boot time.
- `src/util/heap/blockpageresource.rs` — Custom lock-free queue (`BlockQueue`) using `UnsafeCell` and `Option` (refactored from `MaybeUninit`).
- `src/scheduler/gc_work.rs` — Plan casts in `Prepare`/`Release` require raw pointers to avoid UB lint when casting to `&mut`.
- `src/util/memory.rs` — Calls to `libc` functions (`mmap`, `mprotect`, etc.) and safe wrappers around them.
- `src/util/address.rs` — Primitives for raw memory access and address arithmetic.
- `src/util/metadata/vo_bit/mod.rs` — Remaining unsafe is `from_raw_address_unchecked` in `get_object_ref_for_vo_addr` which is irreducible.
- `src/util/heap/layout/map64.rs` — Remaining unsafe are trait signatures and unavoidable `from_usize` calls for reading high water mark.
- `src/util/metadata/safe_access.rs` — Abstraction boundary for `MetadataSlot`.
- `src/util/rust_util/mod.rs` — `InitializeOnce` provides zero-overhead reads for `SFT_MAP` in release builds, requiring unsafe; `libc::gettid()` is FFI.
- `src/util/malloc/malloc_ms_util.rs` — FFI calls to C allocator and raw pointer operations for alignment.
