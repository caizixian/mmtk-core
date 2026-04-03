# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 722 | Current: 586 | Δ: -136
- Phase: 2

## Codebase Invariants (PROTECTED — do not prune)
- `Address` and `ObjectReference` are used to abstract over raw memory.
- `SFTMap::get_unchecked` is now safe and uses bounds checks (or is guaranteed within bounds for `SFTSpaceMap`).
- `SideMetadataOffset` is now a safe `enum` instead of a `union`.

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🟡 MED: `src/util/metadata/side_metadata/helpers.rs:548-557` — Remove unnecessary unsafe blocks (identified by compiler warnings).
2. 🟡 MED: `src/util/metadata/side_metadata/ranges.rs:177` — Remove unnecessary unsafe block.

## Patterns Discovered
- `unsafe { MaybeUninit::uninit().assume_init() }` → `[MaybeUninit::uninit()]` when array size is 1. Works for initializing arrays of `MaybeUninit` safely.
- For arrays of size N where type is not Copy: `[const { MaybeUninit::uninit() }; N]` is safe.
- Using `Address(x)` directly in tests within the same module/submodule to avoid `unsafe` blocks for `Address::from_usize`.
- Safe wrappers in `Mutator` (like `get_allocator_mut_safe`) can encapsulate `unsafe` array access by checking initialization against `space_mapping`.
- Making trait methods safe when implementations can use safe operations (like indexing with bounds checks) even if they might panic on invalid input, to eliminate unsafe at call sites.
- Refactoring `union` to `enum` for types like `SideMetadataOffset` eliminates unsafe field accesses and allows deriving `PartialEq`, `Eq`, and `Hash`.
- Creating an extension trait (e.g., `SideMetadataSpecBlockExt`) to encapsulate unsafe operations on `SideMetadataSpec` for a specific handle type (like `Block`) can reduce unsafe blocks at many call sites.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/util/metadata/side_metadata/helpers.rs` — Irreducible raw loads from metadata addresses. [Phase 1 analysis]
- `src/util/metadata/header_metadata.rs` — Irreducible raw loads from header addresses. [Phase 1 analysis]
- `src/util/alloc/allocators.rs` — Irreducible `assume_init` for layout compatibility with VM bindings. [Phase 1 analysis]
- `src/policy/sft_map.rs` — `SFTRefStorage` uses transmute for atomic fat pointers. [Phase 2 confirmed]

## Abstraction Proposals (for Phase 2)
- Implemented `SideMetadataSpecBlockExt` in `src/policy/marksweepspace/native_ms/block.rs` to abstract metadata accesses.
