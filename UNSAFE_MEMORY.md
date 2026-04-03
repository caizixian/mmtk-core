# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 722 | Current: 702 | Δ: -20
- Phase: 1

## Codebase Invariants (PROTECTED — do not prune)
- None yet.

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🔴 HIGH: `src/util/metadata/helpers.rs` — Audit `Address::from_usize` usages.
2. 🟡 MED: `src/util/metadata/header_metadata.rs` — Analyze high count file for local removals or abstractions.

## Patterns Discovered
- `MaybeUninit` arrays of size 1 can be replaced with `Option` and `unwrap()` to eliminate unsafe access.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/util/copy/mod.rs` — All unsafe removed.

## Abstraction Proposals (for Phase 2)
- None yet.
