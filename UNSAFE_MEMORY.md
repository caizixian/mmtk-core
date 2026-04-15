# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 722 | Current: 721 | Δ: -1
- Phase: 1

## Codebase Invariants (PROTECTED — do not prune)
- `Address::from_ptr` is safe and can be used to replace `Address::from_usize` in non-const contexts to avoid unsafe blocks.

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🔴 HIGH: `src/util/metadata/side_metadata/helpers.rs:293` — Analyze raw loads for safety or abstraction — expected Δ: unknown
2. 🟡 MED: `src/util/copy/mod.rs:92` — Analyze assume_init_mut usage in `CopySelector` — expected Δ: unknown

## Patterns Discovered
- `unsafe { Address::from_usize(x) }` → `Address::from_ptr(x as *const T)` where `x` is a `usize` and context is not `const`.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- None yet.

## Abstraction Proposals (for Phase 2)
- `MetadataSlice` or similar to wrap raw loads/stores in `helpers.rs`.
