# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 722 | Current: 702 (Expected reduction: ~25 in tests) | Δ: -20
- Phase: 1

## Codebase Invariants (PROTECTED — do not prune)
- `Address::from_usize` is marked unsafe by design to warn about invalid addresses. Replacing it with `ZERO.add` is considered an anti-pattern as it is semantically identical.

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🔴 HIGH: `src/util/metadata/header_metadata.rs` — Continue auditing for local removals or move to Phase 2 for safe metadata accessors.
2. 🟡 MED: `src/policy/sft_map.rs` — Analyze for local removals.

## Patterns Discovered
- `MaybeUninit` arrays of size 1 can be replaced with `Option` and `unwrap()` to eliminate unsafe access.
- In tests, raw memory allocation with `alloc_zeroed` and `dealloc` can be replaced with safe `Vec` to eliminate unsafe blocks.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/util/copy/mod.rs` — All unsafe removed.
- `src/util/metadata/side_metadata/helpers.rs` — Remaining unsafe are `from_usize` or raw loads that need abstraction.

## Abstraction Proposals (for Phase 2)
- **Safe Metadata Accessor**: Centralize raw memory access to metadata tables in `header_metadata.rs` and `global.rs`.
