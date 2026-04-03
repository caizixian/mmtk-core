# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 722 | Current: 709 | Δ: -13
- Phase: 1

## Codebase Invariants (PROTECTED — do not prune)
- `Address` and `ObjectReference` are used to abstract over raw memory.

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🔴 HIGH: `src/plan/mutator_context.rs:59` — Audit allocator access and see if unsafe can be removed. — expected Δ: 2
2. 🟡 MED: `src/util/alloc/allocators.rs:46` — Audit `get_allocator` and see if unsafe can be removed. — expected Δ: 2

## Patterns Discovered
- `unsafe { MaybeUninit::uninit().assume_init() }` → `[MaybeUninit::uninit()]` when array size is 1. Works for initializing arrays of `MaybeUninit` safely.
- For arrays of size N where type is not Copy: `[const { MaybeUninit::uninit() }; N]` is safe.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/policy/marksweepspace/native_ms/block.rs` — Irreducible address conversions and raw loads. [Phase 1 analysis]
- `src/util/metadata/side_metadata/helpers.rs` — Irreducible raw loads from metadata addresses. [Phase 1 analysis]
- `src/util/metadata/header_metadata.rs` — Irreducible raw loads from header addresses. [Phase 1 analysis]

## Abstraction Proposals (for Phase 2)
