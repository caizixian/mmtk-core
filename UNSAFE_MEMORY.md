# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 722 | Current: 641 | Δ: -81
- Phase: 2

## Codebase Invariants (PROTECTED — do not prune)
- `Address` and `ObjectReference` are used to abstract over raw memory.
- `SFTMap::get_unchecked` is now safe and uses bounds checks (or is guaranteed within bounds for `SFTSpaceMap`).

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🔴 HIGH: `src/util/heap/monotonepageresource.rs:186-188` — Remove unnecessary unsafe blocks.
2. 🔴 HIGH: `src/util/heap/space_descriptor.rs:101, 111` — Remove unnecessary unsafe blocks.
3. 🔴 HIGH: `src/util/metadata/side_metadata/constants.rs:27` — Remove unnecessary unsafe block.
4. 🔴 HIGH: `src/util/metadata/side_metadata/sanity.rs:382` — Remove unnecessary unsafe block.
5. 🔴 HIGH: `src/util/metadata/side_metadata/side_metadata_tests.rs:42-164` — Remove unnecessary unsafe blocks around Address::from_usize.

## Patterns Discovered
- `unsafe { MaybeUninit::uninit().assume_init() }` → `[MaybeUninit::uninit()]` when array size is 1. Works for initializing arrays of `MaybeUninit` safely.
- For arrays of size N where type is not Copy: `[const { MaybeUninit::uninit() }; N]` is safe.
- Using `Address(x)` directly in tests within the same module/submodule to avoid `unsafe` blocks for `Address::from_usize`.
- Safe wrappers in `Mutator` (like `get_allocator_mut_safe`) can encapsulate `unsafe` array access by checking initialization against `space_mapping`.
- Making trait methods safe when implementations can use safe operations (like indexing with bounds checks) even if they might panic on invalid input, to eliminate unsafe at call sites.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/policy/marksweepspace/native_ms/block.rs` — Irreducible address conversions and raw loads. [Phase 1 analysis]
- `src/util/metadata/side_metadata/helpers.rs` — Irreducible raw loads from metadata addresses. [Phase 1 analysis]
- `src/util/metadata/header_metadata.rs` — Irreducible raw loads from header addresses. [Phase 1 analysis]
- `src/util/alloc/allocators.rs` — Irreducible `assume_init` for layout compatibility with VM bindings. [Phase 1 analysis]
- `src/policy/sft_map.rs` — `SFTRefStorage` uses transmute for atomic fat pointers. [Phase 2 confirmed]

## Abstraction Proposals (for Phase 2)
