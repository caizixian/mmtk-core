# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 722 | Current: 715 | Δ: -7
- Phase: 1

## Codebase Invariants (PROTECTED — do not prune)
- `Address::from_ptr` is safe and can be used to replace `Address::from_usize` in non-const contexts to avoid unsafe blocks.

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🔴 HIGH: `src/util/metadata/side_metadata/helpers.rs:293` — Analyze raw loads for safety or abstraction — expected Δ: unknown
2. 🟡 MED: `src/util/metadata/header_metadata.rs:156` — Analyze raw loads in header metadata — expected Δ: unknown

## Patterns Discovered
- `unsafe { Address::from_usize(x) }` → `Address::from_ptr(x as *const T)` where `x` is a `usize` and context is not `const`.
- Replace `unsafe { MaybeUninit::uninit().assume_init() }` with safe `[MaybeUninit::uninit(); N]` for array initialization when applicable.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/util/copy/mod.rs` — remaining unsafe blocks are `assume_init_mut()` which are likely required for performance to avoid `Option` overhead in GC fast path.

## Abstraction Proposals (for Phase 2)
- `MetadataSlice` or similar to wrap raw loads/stores in `helpers.rs`.
