# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 722 | Current: 691 | Δ: -31
- Phase: 2

## Codebase Invariants (PROTECTED — do not prune)
- `Address` and `ObjectReference` are used to abstract over raw memory.
- `SFTMap::get_unchecked` is now safe and uses bounds checks (or is guaranteed within bounds for `SFTSpaceMap`).

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🔴 HIGH: `src/util/address.rs:158` — Audit why `zero` and `max` are unsafe and if they can be made safe. — expected Δ: 2
2. 🟡 MED: `src/util/address.rs:175` — Audit `from_usize` and see if it can be restricted or made safer. — expected Δ: 2

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
