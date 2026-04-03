# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 722 | Current: 535 | Δ: -187
- Phase: 2

## Codebase Invariants (PROTECTED — do not prune)
- `Address` and `ObjectReference` are used to abstract over raw memory.
- `SFTMap::get_unchecked` is now safe and uses bounds checks (or is guaranteed within bounds for `SFTSpaceMap`).
- `SideMetadataOffset` is now a safe `enum` instead of a `union`.

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🔴 HIGH: `src/policy/marksweepspace/malloc_ms/global.rs:395-850` — Continue investigating remaining unsafe blocks in malloc_ms/global.rs (SFT_MAP, free, load128). — expected Δ: ?

## Patterns Discovered
- `unsafe { MaybeUninit::uninit().assume_init() }` → `[MaybeUninit::uninit()]` when array size is 1. Works for initializing arrays of `MaybeUninit` safely.
- For arrays of size N where type is not Copy: `[const { MaybeUninit::uninit() }; N]` is safe.
- Using `Address(x)` directly in tests within the same module/submodule to avoid `unsafe` blocks for `Address::from_usize`.
- Safe wrappers in `Mutator` (like `get_allocator_mut_safe`) can encapsulate `unsafe` array access by checking initialization against `space_mapping`.
- Making trait methods safe when implementations can use safe operations (like indexing with bounds checks) even if they might panic on invalid input, to eliminate unsafe at call sites.
- Refactoring `union` to `enum` for types like `SideMetadataOffset` eliminates unsafe field accesses and allows deriving `PartialEq`, `Eq`, and `Hash`.
- Creating an extension trait (e.g., `SideMetadataSpecBlockExt`) to encapsulate unsafe operations on `SideMetadataSpec` for a specific handle type (like `Block`) can reduce unsafe blocks at many call sites.
- Replacing `MaybeUninit` with `Option` for arrays of objects that are initialized late allows safe access via `as_mut().expect(...)` and eliminates `assume_init_mut` calls.
- Adding runtime checks (asserts) in `Mutator` to validate that allocators are initialized before accessing them allows removing `unsafe` from accessor methods and callers.
- Using `store_atomic` to replace raw stores in metadata updates, allowing helper functions to be safe and eliminating `unsafe` blocks at call sites.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/util/metadata/side_metadata/helpers.rs` — Irreducible raw loads from metadata addresses. [Phase 1 analysis]
- `src/util/metadata/header_metadata.rs` — Irreducible raw loads from header addresses. [Phase 1 analysis]
- `src/util/alloc/allocators.rs` — Irreducible `assume_init` for layout compatibility with VM bindings. [Phase 1 analysis]
- `src/policy/sft_map.rs` — `SFTRefStorage` uses transmute for atomic fat pointers. [Phase 2 confirmed]
- `src/util/metadata/metadata_val_traits.rs` — Irreducible raw loads from addresses in trait default impls. [Phase 2 confirmed]

## Abstraction Proposals (for Phase 2)
- Implemented `SideMetadataSpecBlockExt` in `src/policy/marksweepspace/native_ms/block.rs` to abstract metadata accesses.
