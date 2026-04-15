# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 535 | Current: 511 | Δ: -24
- Phase: 2

## Codebase Invariants (PROTECTED — do not prune)
- `Address::from_ptr` is safe and can be used to replace `Address::from_usize` in non-const contexts to avoid unsafe blocks.
- `Address::ZERO` is a safe constant that can replace `unsafe { Address::zero() }`.
- `ObjectReference::from_raw_address` is safe and can replace `ObjectReference::from_raw_address_unchecked` when the address is known to be non-zero.

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🔴 HIGH: `src/util/metadata/metadata_val_traits.rs:84-131` — Analyze if trait methods can be made safe.

## Patterns Discovered
- `unsafe { Address::from_usize(x) }` → `Address::from_ptr(x as *const T)` where `x` is a `usize` and context is not `const`.
- Replace `unsafe { MaybeUninit::uninit().assume_init() }` with safe `[const { MaybeUninit::uninit() }; N]` for array initialization when the type is not `Copy`.
- `unsafe { Address::zero() }` → `Address::ZERO`.
- `unsafe { ObjectReference::from_raw_address_unchecked(x) }` → `ObjectReference::from_raw_address(x).unwrap()` when `x` is known to be non-zero.
- Use `MetadataCursor` to encapsulate raw memory access for `MetadataValue` types, centralizing unsafe operations.
- Inline `MetadataCursor` loads with masking for sub-byte metadata to remove unsafe blocks in search functions.
- Use `Vec` instead of manual allocation in tests to eliminate unsafe blocks.
- Replace `UnsafeCell` with `RwLock` in global maps (Map32, Map64) to eliminate unsafe operations.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/util/copy/mod.rs` — remaining unsafe blocks are `assume_init_mut()` which are likely required for performance to avoid `Option` overhead in GC fast path.
- `src/policy/sft_map.rs` — `transmute` of fat pointers is required for atomic trait object storage in `SFTRefStorage`. [Phase 2 confirmed]
- `src/util/malloc/malloc_ms_util.rs` — mostly FFI calls to `libc` (malloc, calloc, free, etc.). [Phase 2 confirmed]
- `src/util/alloc/allocators.rs` — Layout constraints for VM bindings require `MaybeUninit` arrays with separate initialization flags. `assume_init_ref` and `assume_init_mut` are required and safe due to runtime checks, but must be marked unsafe by compiler. [Phase 2 confirmed]
- `src/util/metadata/header_metadata.rs` — Production unsafe is irreducible (raw address access in `load`/`store`), tests refactored to use `Vec`. [Phase 2 confirmed]
- `src/vm/tests/mock_tests/mock_test_slots.rs` — Remaining unsafe blocks are dereferencing raw pointers to simulate VM slots and `unsafe impl Send`. [Phase 2 confirmed]
- `src/util/metadata/side_metadata/global.rs` — Production unsafe in `load`/`store` is irreducible due to concurrent access invariants requiring `unsafe fn` signature. [Phase 2 confirmed]

## Abstraction Proposals (for Phase 2)
- `SweepProof` for `malloc_ms` (Implemented).
- `MetadataCursor` to wrap raw loads/stores (Implemented in `helpers.rs`, extended with generic methods).
