# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 535 | Current: 435 | Δ: -100
- Phase: 2

## Codebase Invariants (PROTECTED — do not prune)
- `Address::from_ptr` is safe and can be used to replace `Address::from_usize` in non-const contexts to avoid unsafe blocks.
- `Address::ZERO` is a safe constant that can replace `unsafe { Address::zero() }`.
- `ObjectReference::from_raw_address` is safe and can replace `ObjectReference::from_raw_address_unchecked` when the address is known to be non-zero.

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🔴 HIGH: `src/util/alloc/free_list_allocator.rs:155-409` — analyze unsafe blocks in free list allocator — expected Δ: 0-3

## Patterns Discovered
- `unsafe { Address::from_usize(x) }` → `Address::from_ptr(x as *const T)` where `x` is a `usize` and context is not `const`.
- Replace `unsafe { MaybeUninit::uninit().assume_init() }` with safe `[const { MaybeUninit::uninit() }; N]` for array initialization when the type is not `Copy`.
- `unsafe { Address::zero() }` → `Address::ZERO`.
- `unsafe { ObjectReference::from_raw_address_unchecked(x) }` → `ObjectReference::from_raw_address(x).unwrap()` when `x` is known to be non-zero.
- Use `MetadataCursor` to encapsulate raw memory access for `MetadataValue` types, centralizing unsafe operations.
- Inline `MetadataCursor` loads with masking for sub-byte metadata to remove unsafe blocks in search functions.
- Use `Vec` instead of manual allocation in tests to eliminate unsafe blocks.
- Replace `UnsafeCell` with `RwLock` in global maps (Map32, Map64) to eliminate unsafe operations.
- Use `load_atomic`/`store_atomic` with `Relaxed` ordering for full-byte metadata in non-atomic contexts (guaranteed by `SweepProof`) to eliminate unsafe blocks without performance penalty.
- Removing `unsafe` from function signatures when the body contains no unsafe operations and preconditions are enforced by types (e.g., `MutexGuard` or `&mut` references).
- Replace `*mut c_void` with `usize` in opaque pointer types to eliminate `unsafe impl Send` and `Sync`.
- Use `std::sync::OnceLock` instead of `MaybeUninit` for single-assignment fields in shared structures to eliminate unsafe initialization and access (e.g., in `GCTrigger`).
- Use `Box::leak` instead of `Box::into_raw` to get a reference directly when initializing lock-free structures, reducing unsafe blocks.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/util/rust_util/atomic_box.rs` — implements a safe abstraction (`OnceOptionBox`). Unsafe is required for raw pointer manipulation and justified `Zeroable` impl. [Phase 2 confirmed]
- `src/util/copy/mod.rs` — remaining unsafe blocks are `assume_init_mut()` which are likely required for performance to avoid `Option` overhead in GC fast path.
- `src/policy/sft_map.rs` — `transmute` of fat pointers is required for atomic trait object storage in `SFTRefStorage`. [Phase 2 confirmed]
- `src/util/malloc/malloc_ms_util.rs` — mostly FFI calls to `libc` (malloc, calloc, free, etc.). [Phase 2 confirmed]
- `src/util/alloc/allocators.rs` — Layout constraints for VM bindings require `MaybeUninit` arrays with separate initialization flags. `assume_init_ref` and `assume_init_mut` are required and safe due to runtime checks, but must be marked unsafe by compiler. [Phase 2 confirmed]
- `src/util/metadata/header_metadata.rs` — Production unsafe is irreducible (raw address access in `load`/`store`), tests refactored to use `Vec`. [Phase 2 confirmed]
- `src/vm/tests/mock_tests/mock_test_slots.rs` — Remaining unsafe blocks are dereferencing raw pointers to simulate VM slots and `unsafe impl Send`. [Phase 2 confirmed]
- `src/util/metadata/side_metadata/global.rs` — Production unsafe in `load`/`store` is irreducible due to concurrent access invariants requiring `unsafe fn` signature. [Phase 2 confirmed]
- `src/util/metadata/metadata_val_traits.rs` — Trait methods must remain unsafe because they take a raw `Address` and dereference it. [Phase 2 confirmed]
- `src/util/metadata/side_metadata/side_metadata_tests.rs` — Remaining unsafe blocks in tests require complex bit extraction for sub-byte metadata. [Phase 2 confirmed]
- `src/util/address.rs` — Remaining unsafe blocks are `load`, `store`, `as_ref`, etc., which are irreducible as they dereference raw pointers. [Phase 2 confirmed]
- `src/policy/marksweepspace/malloc_ms/metadata.rs` — Remaining unsafe is `u128` load (primitive not implementing `MetadataValue`) and `SweepProof` constructor. [Phase 2 confirmed]
- `src/util/metadata/vo_bit/mod.rs` — `find_prev_non_zero_value` is encapsulated in safe `find_object_from_internal_pointer`. Irreducible without capability tokens. [Phase 2 confirmed]
- `src/util/heap/freelistpageresource.rs` — Remaining unsafe are `unsafe impl Send` and `unsafe impl Sync`. [Phase 2 confirmed]
- `src/util/rust_util/mod.rs` — `InitializeOnce` avoids checks on reads for performance, and `get_mut` allows mutating from `&self` which is necessary for plan creation but inherently unsafe. [Phase 2 confirmed]
- `src/vm/slot.rs` — `SimpleSlot` and `Address` as `Slot` require raw pointer dereference to avoid lifetimes in the `Slot` trait. [Phase 2 confirmed]
- `src/util/conversions.rs` — All unsafe blocks removed. [Phase 2 confirmed]
- `src/util/memory.rs` — Irreducible FFI calls (`mmap`, `madvise`, `munmap`, `mprotect`) and core primitives (`ptr::write_bytes`). [Phase 2 confirmed]
- `src/util/metadata/side_metadata/helpers.rs` — Implementation of `MetadataCursor` abstraction, irreducible without moving unsafe to call sites. [Phase 2 confirmed]
- `docs/dummyvm/src/api.rs` — Irreducible FFI boundary operations (raw pointer dereferencing and Box::from_raw). [Phase 2 confirmed]

## Abstraction Proposals (for Phase 2)
- `SweepProof` for `malloc_ms` (Implemented).
- `MetadataCursor` to wrap raw loads/stores (Implemented in `helpers.rs`, extended with generic methods).
