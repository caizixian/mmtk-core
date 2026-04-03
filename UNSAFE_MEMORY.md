# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 722 | Current: 513 | Δ: -209
- Phase: 2

## Codebase Invariants (PROTECTED — do not prune)
- `Address` and `ObjectReference` are used to abstract over raw memory.
- `SFTMap::get_unchecked` is now safe and uses bounds checks (or is guaranteed within bounds for `SFTSpaceMap`).
- `SideMetadataOffset` is now a safe `enum` instead of a `union`.

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🔴 HIGH: `src/util/heap/layout/map64.rs` — Apply same refactoring as `map32.rs` (replace `UnsafeCell` with `Mutex`) — expected Δ: ~6
2. 🟡 MED: `src/util/metadata/side_metadata/global.rs:200-800` — Investigate raw loads/stores and `addr.as_ref` in side_metadata/global.rs. — expected Δ: ?

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
- Using references instead of raw pointers in test slots when the slots borrow from local variables in tests. This eliminates unsafe dereferences and `unsafe impl Send`.
- **New Pattern**: Replacing non-atomic `load`/`store` on `SideMetadataSpec` with `load_atomic`/`store_atomic` (with `Relaxed` or `SeqCst`) to remove `unsafe` blocks at call sites.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/util/metadata/side_metadata/helpers.rs` — Irreducible raw loads from metadata addresses. [Phase 1 analysis]
- `src/util/metadata/header_metadata.rs` — Irreducible raw loads from header addresses. [Phase 1 analysis]
- `src/util/alloc/allocators.rs` — Irreducible `assume_init` for layout compatibility with VM bindings. [Phase 1 analysis]
- `src/policy/sft_map.rs` — `SFTRefStorage` uses transmute for atomic fat pointers. [Phase 2 confirmed]
- `src/util/metadata/metadata_val_traits.rs` — Irreducible raw loads from addresses in trait default impls. [Phase 2 confirmed]
- `src/vm/tests/mock_tests/mock_test_slots.rs` — All unsafe removed by refactoring to use references in tests. [Phase 2 confirmed]
- `src/policy/marksweepspace/native_ms/block.rs` — Remaining unsafe are irreducible raw memory accesses for free list and raw pointer dereferences. [Phase 2 confirmed]
- `src/util/heap/layout/map32.rs` — Remaining unsafe are trait methods that must match the unsafe trait definition. [Phase 2 confirmed]

## Abstraction Proposals (for Phase 2)
- Implemented `SideMetadataSpecBlockExt` in `src/policy/marksweepspace/native_ms/block.rs` to abstract metadata accesses.
