# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 722 | Current: 621 | Δ: -2 (Merged blocks in header_metadata.rs)
- Phase: 2

## Codebase Invariants (PROTECTED — do not prune)
- `Address::from_ptr` is safe and can be used to replace `Address::from_usize` in non-const contexts to avoid unsafe blocks.
- `Address::ZERO` is a safe constant that can replace `unsafe { Address::zero() }`.
- `ObjectReference::from_raw_address` is safe and can replace `ObjectReference::from_raw_address_unchecked` when the address is known to be non-zero.

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🔴 HIGH: `src/util/metadata/side_metadata/global.rs:56` — Analyze raw heap access in side metadata — expected Δ: large
2. 🟡 MED: `src/util/metadata/side_metadata/helpers.rs:293` — Analyze raw loads in side metadata helpers (likely requires MetadataSlice abstraction) — expected Δ: unknown

## Patterns Discovered
- `unsafe { Address::from_usize(x) }` → `Address::from_ptr(x as *const T)` where `x` is a `usize` and context is not `const`.
- Replace `unsafe { MaybeUninit::uninit().assume_init() }` with safe `[const { MaybeUninit::uninit() }; N]` for array initialization when the type is not `Copy`.
- `unsafe { Address::zero() }` → `Address::ZERO`.
- `unsafe { ObjectReference::from_raw_address_unchecked(x) }` → `ObjectReference::from_raw_address(x).unwrap()` when `x` is known to be non-zero.
- Replace `self.sft.get_unchecked(index)` with normal indexing `self.sft[index]` when bounds are guaranteed by construction.
- Use proof tokens (e.g., `SweepProof`) to encapsulate unsafe non-atomic operations when exclusive access is guaranteed by the phase.
- **New**: Merge contiguous-ish unsafe blocks in `if/else` branches to reduce unsafe block count.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/util/copy/mod.rs` — remaining unsafe blocks are `assume_init_mut()` which are likely required for performance to avoid `Option` overhead in GC fast path.
- `src/util/metadata/header_metadata.rs` — remaining unsafe blocks are raw memory operations (load/store/atomic) that are required for metadata access. Merged blocks where possible. [Phase 2 confirmed]

## Abstraction Proposals (for Phase 2)
- `SweepProof` for `malloc_ms` (Implemented).
- `MetadataSlice` or similar to wrap raw loads/stores in `helpers.rs`.
