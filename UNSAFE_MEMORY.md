# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 722 | Current: 666 | Δ: -56
- Phase: 1

## Codebase Invariants (PROTECTED — do not prune)
- `Address::from_usize` is marked unsafe by design to warn about invalid addresses. Replacing it with `ZERO.add` is considered an anti-pattern as it is semantically identical.

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🔴 HIGH: `src/util/metadata/header_metadata.rs` — Tests refactored to use slices. Production code still needs Phase 2 abstraction for safe metadata access.
2. 🟡 MED: `src/util/metadata/side_metadata/global.rs` — Refactored `SideMetadataOffset` to enum removing 10 unsafe blocks. Remaining unsafe needs Phase 2 abstraction.

## Patterns Discovered
- `MaybeUninit` arrays of size 1 can be replaced with `Option` and `unwrap()` to eliminate unsafe access.
- In tests, raw memory allocation with `alloc_zeroed` and `dealloc` can be replaced with safe `Vec` to eliminate unsafe blocks.
- `unsafe impl Sync` for types containing only atomics or Sync types can often be removed as the compiler can auto-derive Sync.
- `get_unchecked` on `Vec` can be replaced with standard indexing `[]` if we are okay with bounds checks (or if bounds are already checked).
- Using safe slices `&mut [T]` instead of raw pointers `*mut T` in tests allows using safe indexing and removes unsafe dereferences.
- **Refactoring unions to enums** can eliminate unsafe field accesses if layout compatibility is not strictly required or if the overhead is acceptable.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/util/copy/mod.rs` — All unsafe removed.
- `src/util/metadata/side_metadata/helpers.rs` — Remaining unsafe are `from_usize` or raw loads that need abstraction.
- `src/policy/sft_map.rs` — Remaining unsafe are trait signatures and unavoidable transmutes for fat pointers in atomics.

## Abstraction Proposals (for Phase 2)
- **Safe Metadata Accessor**: Centralize raw memory access to metadata tables in `header_metadata.rs` and `global.rs`.
