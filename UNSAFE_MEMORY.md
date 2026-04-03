# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 722 | Current: 441 | Δ: -281
- Phase: 2

## Codebase Invariants (PROTECTED — do not prune)
- `Address` and `ObjectReference` are used to abstract over raw memory.
- `SFTMap::get_unchecked` is now safe and uses bounds checks (or is guaranteed within bounds for `SFTSpaceMap`).
- `SideMetadataOffset` is now a safe `enum` instead of a `union`.

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🔴 HIGH: `src/mmtk.rs:187,189` — Check if raw pointer casts for plan/gc_trigger can be removed or made safe. — expected Δ: 2

## Patterns Discovered
- Introducing `MetadataSlot` abstraction to encapsulate raw memory operations on metadata addresses behind a safe API.
- Making `MetadataSlot` `pub(crate)` and adding `load_non_atomic` and `store_non_atomic` allows reusing it across modules (e.g., in `header_metadata.rs`).
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
- Replacing non-atomic `load`/`store` on `SideMetadataSpec` with `load_atomic`/`store_atomic` (with `Relaxed` or `SeqCst`) to remove `unsafe` blocks at call sites.
- Replacing `UnsafeCell` with `Mutex` for global state that is accessed via shared references, eliminating unsafe mutable access.
- **New Pattern**: Extending `MetadataSlot` with generic methods for `MetadataValue` allows centralizing unsafe operations on types larger than `u8` (like `u16`, `u32`, `usize`) and removing unsafe blocks at call sites in `header_metadata.rs` and `global.rs`.
- **New Pattern**: Removing raw pointers from work packets and using `mmtk.get_plan_mut()` eliminates the need for `unsafe impl Send` and raw pointer casts when the work packet only needs to call trait methods on the plan.
- **New Pattern**: Introduce `FreeListCell` abstraction to encapsulate raw memory operations on free list cells.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/util/metadata/side_metadata/helpers.rs` — All unsafe removed by using `MetadataSlot`. [Phase 2 confirmed]
- `src/util/metadata/header_metadata.rs` — Irreducible raw loads from header addresses. [Phase 1 analysis] (Re-evaluated in Phase 2, used MetadataSlot for some reductions)
- `src/util/alloc/allocators.rs` — Irreducible `assume_init` for layout compatibility with VM bindings. [Phase 1 analysis]
- `src/policy/sft_map.rs` — `SFTRefStorage` uses transmute for atomic fat pointers. [Phase 2 confirmed]
- `src/util/metadata/metadata_val_traits.rs` — Irreducible raw loads from addresses in trait default impls. [Phase 2 confirmed]
- `src/vm/tests/mock_tests/mock_test_slots.rs` — All unsafe removed by refactoring to use references in tests. [Phase 2 confirmed]
- `src/policy/marksweepspace/native_ms/block.rs` — Remaining unsafe are irreducible raw memory accesses for free list and raw pointer dereferences. [Phase 2 confirmed]
- `src/util/heap/layout/map32.rs` — Remaining unsafe are trait methods that must match the unsafe trait definition. [Phase 2 confirmed]
- `src/util/rust_util/mod.rs` — `InitializeOnce` is a custom optimization for `SFT_MAP` to avoid checks on reads. Remaining unsafe in `gettid` is platform-specific. [Phase 2 confirmed]
- `src/util/metadata/side_metadata/side_metadata_tests.rs` — Remaining unsafe are irreducible raw memory accesses and allocation in tests. [Phase 2 confirmed]
- `src/util/metadata/side_metadata/global.rs` — Remaining unsafe are irreducible function signatures and raw memory copy. [Phase 2 confirmed]
- `src/util/heap/blockpageresource.rs` — Remaining unsafe are irreducible UnsafeCell accesses in lock-free queue. [Phase 2 confirmed]
- `src/util/malloc/malloc_ms_util.rs` — Irreducible FFI and raw pointer manipulation. [Phase 2 confirmed]
- `src/policy/marksweepspace/malloc_ms/metadata.rs` — Remaining unsafe are non-atomic performance optimizations or low-level primitives (`load128`). [Phase 2 confirmed]
- `docs/dummyvm/src/api.rs` — Irreducible FFI boundaries in dummy VM implementation. [Phase 2 confirmed]
- `src/util/memory.rs` — Contains wrappers for FFI calls. The unsafe blocks are the FFI calls themselves. [Phase 2 confirmed]
- `src/util/address.rs` — Primitives for address operations. Unsafe signatures are necessary. [Phase 2 confirmed]
- `src/vm/slot.rs` — `SimpleSlot` is a safe abstraction. Unsafe operations inside it are irreducible without viral lifetimes. Tests use unsafe to check address iteration. [Phase 2 confirmed]
- `src/util/heap/freelistpageresource.rs` — Remaining unsafe are `Send`/`Sync` impls for the type. [Phase 2 confirmed]
- `src/util/alloc/free_list_allocator.rs` — Remaining unsafe is irreducible ObjectReference creation from raw address. [Phase 2 confirmed]

## Abstraction Proposals (for Phase 2)
- Implemented `SideMetadataSpecBlockExt` in `src/policy/marksweepspace/native_ms/block.rs` to abstract metadata accesses.
- Extended `MetadataSlot` in `src/util/metadata/side_metadata/global.rs` to support generic `MetadataValue` operations.
