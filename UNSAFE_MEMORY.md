# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 722 | Current: 715 | Δ: -7
- Phase: 1

## Codebase Invariants (PROTECTED — do not prune)
- `Address` and `ObjectReference` are used to abstract over raw memory.

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🔴 HIGH: `src/policy/marksweepspace/native_ms/block.rs:48` — Check if this can be made safe or if it's irreducible. — expected Δ: 1
2. 🟡 MED: `src/util/metadata/side_metadata/helpers.rs:293` — Audit load/store operations. — expected Δ: 1
3. 🟢 LOW: `src/util/metadata/header_metadata.rs:128` — Audit header metadata loads. — expected Δ: 1

## Patterns Discovered
- `unsafe { MaybeUninit::uninit().assume_init() }` → `[MaybeUninit::uninit()]` when array size is 1. Works for initializing arrays of `MaybeUninit` safely.

## Files NOT to Revisit (all remaining unsafe is irreducible)

## Abstraction Proposals (for Phase 2)
