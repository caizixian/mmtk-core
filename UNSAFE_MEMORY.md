# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 722 | Current: 683 | Δ: -39
- Phase: 1

## Codebase Invariants (PROTECTED — do not prune)
- `Address::from_ptr` is safe and can be used to replace `Address::from_usize` in non-const contexts to avoid unsafe blocks.
- `Address::ZERO` is a safe constant that can replace `unsafe { Address::zero() }`.
- `ObjectReference::from_raw_address` is safe and can replace `ObjectReference::from_raw_address_unchecked` when the address is known to be non-zero.

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🔴 HIGH: `src/util/metadata/header_metadata.rs:156` — Analyze raw loads in header metadata — expected Δ: unknown
2. 🟡 MED: `src/policy/marksweepspace/native_ms/block.rs:300` — Address remaining raw stores to cell memory for free list — expected Δ: unknown

## Patterns Discovered
- `unsafe { Address::from_usize(x) }` → `Address::from_ptr(x as *const T)` where `x` is a `usize` and context is not `const`.
- Replace `unsafe { MaybeUninit::uninit().assume_init() }` with safe `[MaybeUninit::uninit(); N]` for array initialization when applicable.
- `unsafe { Address::zero() }` → `Address::ZERO`.
- `unsafe { ObjectReference::from_raw_address_unchecked(x) }` → `ObjectReference::from_raw_address(x).unwrap()` when `x` is known to be non-zero.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/util/copy/mod.rs` — remaining unsafe blocks are `assume_init_mut()` which are likely required for performance to avoid `Option` overhead in GC fast path.

## Abstraction Proposals (for Phase 2)
- `MetadataSlice` or similar to wrap raw loads/stores in `helpers.rs`.
