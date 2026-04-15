# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 722 | Current: 577 | Δ: +1 (Introduced MetadataCursor atomic methods, removed 3 blocks in global.rs, added 4 in helpers.rs)
- Phase: 2

## Codebase Invariants (PROTECTED — do not prune)
- `Address::from_ptr` is safe and can be used to replace `Address::from_usize` in non-const contexts to avoid unsafe blocks.
- `Address::ZERO` is a safe constant that can replace `unsafe { Address::zero() }`.
- `ObjectReference::from_raw_address` is safe and can replace `ObjectReference::from_raw_address_unchecked` when the address is known to be non-zero.

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🔴 HIGH: `src/util/metadata/header_metadata.rs:120-200` — Investigate if `MetadataCursor` or a similar abstraction can be used to reduce unsafe blocks in header metadata access. — expected Δ: unknown

## Patterns Discovered
- `unsafe { Address::from_usize(x) }` → `Address::from_ptr(x as *const T)` where `x` is a `usize` and context is not `const`.
- Replace `unsafe { MaybeUninit::uninit().assume_init() }` with safe `[const { MaybeUninit::uninit() }; N]` for array initialization when the type is not `Copy`.
- `unsafe { Address::zero() }` → `Address::ZERO`.
- `unsafe { ObjectReference::from_raw_address_unchecked(x) }` → `ObjectReference::from_raw_address(x).unwrap()` when `x` is known to be non-zero.
- Replace `self.sft.get_unchecked(index)` with normal indexing `self.sft[index]` when bounds are guaranteed by construction.
- Use proof tokens (e.g., `SweepProof`) to encapsulate unsafe non-atomic operations when exclusive access is guaranteed by the phase.
- Introduce internal cursor/wrapper types (like `MetadataCursor`) to encapsulate repetitive raw memory loads/stores and reduce unsafe blocks at call sites.
- Track initialization state of `MaybeUninit` arrays with boolean flags or bitmasks to allow safe retrieval methods that panic on uninitialized access instead of being `unsafe`.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/util/copy/mod.rs` — remaining unsafe blocks are `assume_init_mut()` which are likely required for performance to avoid `Option` overhead in GC fast path.
- `src/policy/sft_map.rs` — `transmute` of fat pointers is required for atomic trait object storage in `SFTRefStorage`. [Phase 2 confirmed]
- `src/util/malloc/malloc_ms_util.rs` — mostly FFI calls to `libc` (malloc, calloc, free, etc.). [Phase 2 confirmed]
- `src/util/alloc/allocators.rs` — Layout constraints for VM bindings require `MaybeUninit` arrays with separate initialization flags. `assume_init_ref` and `assume_init_mut` are required and safe due to runtime checks, but must be marked unsafe by compiler. Merged blocks to reduce count. [Phase 2 confirmed]

## Abstraction Proposals (for Phase 2)
- `SweepProof` for `malloc_ms` (Implemented).
- `MetadataCursor` to wrap raw loads/stores in `helpers.rs` (Implemented, extended with atomic methods).
