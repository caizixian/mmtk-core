# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 535 | Current: 364 | Δ: -171
- Phase: 2

## Codebase Invariants (PROTECTED — do not prune)
- `Address::from_ptr` is safe and can be used to replace `Address::from_usize` in non-const contexts to avoid unsafe blocks.
- `Address::ZERO` is a safe constant that can replace `unsafe { Address::zero() }`.
- `ObjectReference::from_raw_address` is safe and can replace `ObjectReference::from_raw_address_unchecked` when the address is known to be non-zero.

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🔴 HIGH: `src/policy/lockfreeimmortalspace.rs:130` — Investigate if `eager_initialize` can be made safe or if we can add a safety comment.

## Patterns Discovered
- `unsafe { Address::from_usize(x) }` → `Address::from_ptr(x as *const T)` where `x` is a `usize` and context is not `const`.
- Replace `unsafe { MaybeUninit::uninit().assume_init() }` with safe `[const { MaybeUninit::uninit() }; N]` for array initialization when the type is not `Copy`.
- `unsafe { Address::zero() }` → `Address::ZERO`.
- `unsafe { ObjectReference::from_raw_address_unchecked(x) }` → `ObjectReference::from_raw_address(x).unwrap()` when `x` is known to be non-zero.
- Use `MetadataCursor` to encapsulate raw memory access for `MetadataValue` types, centralizing unsafe operations.
- Inline `MetadataCursor` loads with masking for sub-byte metadata to remove unsafe blocks in search functions.
- Use `Vec` and slice indexing instead of raw pointer dereferencing in tests to eliminate unsafe blocks (e.g., in `header_metadata.rs`).
- Replace `UnsafeCell` with `RwLock` in global maps (Map32, Map64) to eliminate unsafe operations.
- Use `load_atomic`/`store_atomic` with `Relaxed` ordering for full-byte metadata in non-atomic contexts (guaranteed by `SweepProof`) to eliminate unsafe blocks without performance penalty.
- Removing `unsafe` from function signatures when the body contains no unsafe operations and preconditions are enforced by types (e.g., `MutexGuard` or `&mut` references).
- Replace `*mut c_void` with `usize` in opaque pointer types to eliminate `unsafe impl Send` and `Sync`.
- Use `std::sync::OnceLock` instead of `MaybeUninit` for single-assignment fields in shared structures to eliminate unsafe initialization and access (e.g., in `GCTrigger`).
- Use `Box::leak` instead of `Box::into_raw` to get a reference directly when initializing lock-free structures, reducing unsafe blocks.
- Replace `[MaybeUninit<T>; N]` with `[Option<T>; N]` for lazily initialized arrays if N is small or overhead is acceptable, eliminating `assume_init_mut()` unsafe calls.
- Refactor mock slots in tests to use references instead of raw pointers to eliminate unsafe blocks in tests.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/util/copy/mod.rs` — All unsafe blocks removed by replacing MaybeUninit with Option. [Phase 2 confirmed]
- `src/util/rust_util/atomic_box.rs` — implements a safe abstraction (`OnceOptionBox`). Unsafe is required for raw pointer manipulation and justified `Zeroable` impl. [Phase 2 confirmed]
- `src/policy/sft_map.rs` — `transmute` of fat pointers is required for atomic trait object storage in `SFTRefStorage`. [Phase 2 confirmed]
- `src/util/malloc/malloc_ms_util.rs` — mostly FFI calls to `libc` (malloc, calloc, free, etc.). [Phase 2 confirmed]
- `src/util/alloc/allocators.rs` — Layout constraints for VM bindings require `MaybeUninit` arrays with separate initialization flags. `assume_init_ref` and `assume_init_mut` are required and safe due to runtime checks, but must be marked unsafe by compiler. [Phase 2 confirmed]
- `src/util/metadata/header_metadata.rs` — Production unsafe is irreducible (raw address access in `load`/`store`), tests refactored to use `Vec` and slice indexing. [Phase 2 confirmed]
- `src/util/metadata/side_metadata/global.rs` — Production unsafe in `load`/`store` is irreducible due to concurrent access invariants requiring `unsafe fn` signature. [Phase 2 confirmed]
- `src/util/metadata/metadata_val_traits.rs` — Trait methods must remain unsafe because they take a raw `Address` and dereference it. [Phase 2 confirmed]
- `src/util/metadata/side_metadata/side_metadata_tests.rs` — Remaining unsafe blocks in tests require complex bit extraction for sub-byte metadata. [Phase 2 confirmed]
- `src/util/address.rs` — Remaining unsafe blocks are `load`, `store`, `as_ref`, etc., which are irreducible as they dereference raw pointers. [Phase 2 confirmed]
- `src/policy/marksweepspace/malloc_ms/metadata.rs` — Remaining unsafe is `u128` load (primitive not implementing `MetadataValue`) and `SweepProof` constructor. [Phase 2 confirmed]
- `src/util/metadata/vo_bit/mod.rs` — `find_prev_non_zero_value` is encapsulated in safe `find_object_from_internal_pointer`. Irreducible without capability tokens. [Phase 2 confirmed]
- `src/util/heap/freelistpageresource.rs` — Remaining unsafe are `unsafe impl Send` and `unsafe impl Sync`. [Phase 2 confirmed]
- `src/util/rust_util/mod.rs` — `InitializeOnce` avoids checks on reads for performance, and `get_mut` allows mutating from `&self` which is necessary for plan creation but inherently unsafe. Attempt to use `OnceLock` failed due to UB in reference casting. [Phase 2 confirmed]
- `src/vm/slot.rs` — `SimpleSlot` and `Address` as `Slot` require raw pointer dereference to avoid lifetimes in the `Slot` trait. [Phase 2 confirmed]
- `src/util/conversions.rs` — All unsafe blocks removed. [Phase 2 confirmed]
- `src/util/memory.rs` — Irreducible FFI calls (`mmap`, `madvise`, `munmap`, `mprotect`) and core primitives (`ptr::write_bytes`). [Phase 2 confirmed]
- `src/util/metadata/side_metadata/helpers.rs` — Implementation of `MetadataCursor` abstraction, irreducible without moving unsafe to call sites. [Phase 2 confirmed]
- `docs/dummyvm/src/api.rs` — Irreducible FFI boundary operations (raw pointer dereferencing and Box::from_raw). [Phase 2 confirmed]
- `src/util/alloc/free_list_allocator.rs` — Remaining unsafe blocks are raw heap access for free list manipulation. [Phase 2 confirmed]
- `src/policy/immix/immixspace.rs` — All unsafe blocks removed or moved to `schedule_collection` in previous steps. [Phase 2 confirmed]
- `src/mmtk.rs` — Unsafe required for casting local plan to static reference for `gc_trigger` and dereferencing `UnsafeCell`. [Phase 2 confirmed]
- `src/util/heap/blockpageresource.rs` — All unsafe blocks removed by replacing UnsafeCell with RwLock in BlockQueue. [Phase 2 confirmed]
- `src/vm/tests/mock_tests/mock_test_slots.rs` — All unsafe blocks removed by refactoring tests to use references instead of raw pointers. [Phase 2 confirmed]
- `src/util/heap/layout/map64.rs` — Redundant unsafe impl Send and Sync removed. [Phase 2 confirmed]
- `src/util/int_array_freelist.rs` — All unsafe blocks removed by replacing raw pointer with `Arc<RwLock>`. [Phase 2 confirmed]
- `src/util/malloc/mod.rs` — Irreducible FFI calls to library `malloc`, `calloc`, `realloc`, `free`. [Phase 2 confirmed]
- `src/scheduler/affinity.rs` — Irreducible FFI calls to libc for thread affinity (`sched_getaffinity`, `sched_setaffinity`). [Phase 2 confirmed]
- `src/policy/copyspace.rs` — Irreducible FFI calls to `mprotect` and unsafe cast to extend lifetime for `CopySpace` reference. [Phase 2 confirmed]
- `src/policy/marksweepspace/malloc_ms/global.rs` — Irreducible FFI calls to `free` and unsafe cast to extend lifetime for work packets. [Phase 2 confirmed]
- `src/util/alloc/allocator.rs` — Irreducible `unsafe impl Sync` for `AllocationOptionsHolder` and `ptr::write_bytes` for alignment gap filling. [Phase 2 confirmed]
- `src/util/test_util/mock_vm.rs` — Irreducible `transmute` in `lifetime!` macro to remove lifetimes in mock VM for testing. [Phase 2 confirmed]

## Abstraction Proposals (for Phase 2)
- `SweepProof` for `malloc_ms` (Implemented).
- `MetadataCursor` to wrap raw loads/stores (Implemented in `helpers.rs`, extended with generic methods).
- `ExclusivePlanAccessProof` token for safe mutable plan access during global phases (e.g., Prepare and Release).
- `ProofCell` to enforce access control with proof tokens (Implemented in `rust_util/mod.rs`).
