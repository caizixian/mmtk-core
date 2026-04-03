# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 531 | Current: 512 | Δ: -19
- Phase: 1

## Codebase Invariants (PROTECTED — do not prune)
- `Address::from_usize` is marked unsafe by design to warn about invalid addresses. Replacing it with `ZERO.add` is considered an anti-pattern as it is semantically identical.

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🔴 HIGH: Analyze `src/scheduler/gc_work.rs` for applying Typestate pattern to remove unsafe plan casts.
2. 🟡 MED: Explore other files for Phase 1 reductions.

## Patterns Discovered
- `MaybeUninit` arrays of size 1 can be replaced with `Option` and `unwrap()` to eliminate unsafe access.
- In tests, raw memory allocation with `alloc_zeroed` and `dealloc` can be replaced with safe `Vec` to eliminate unsafe blocks.
- `unsafe impl Sync` for types containing only atomics or Sync types can often be removed as the compiler can auto-derive Sync.
- `get_unchecked` on `Vec` can be replaced with standard indexing `[]` if we are okay with bounds checks (or if bounds are already checked).
- Using safe slices `&mut [T]` instead of raw pointers `*mut T` in tests allows using safe indexing and removes unsafe dereferences.
- **Refactoring unions to enums**: can eliminate unsafe field accesses if layout compatibility is not strictly required or if the overhead is acceptable.
- `NonZeroUsize::new_unchecked` can be replaced with `NonZeroUsize::new().expect()` if the value is known to be non-zero.
- **MetadataSlot**: Centralizes unsafe raw memory access in `metadata_val_traits.rs`.
- **Safe Constructor**: Adding `slot_for` to `SideMetadataSpec` and `HeaderMetadataSpec` allows safe access to `MetadataSlot` without unsafe blocks at call sites.
- In tests, `unsafe { Address::from_usize(0) }` can be replaced with the safe constant `Address::ZERO`.
- **Safe initialization of MaybeUninit arrays**: Use `[const { MaybeUninit::uninit() }; N]` instead of `unsafe { MaybeUninit::uninit().assume_init() }`.
- In tests, `SideMetadataSpec::load/store` can be replaced with `load_atomic/store_atomic` with `Ordering::Relaxed` to eliminate unsafe blocks, provided the test does not specifically require non-atomic operations.
- **Refactoring to use MetadataSlot**: Using `self.slot_for` or `MetadataSlot` methods can eliminate unsafe blocks in `SideMetadataSpec` methods like `compare_exchange_atomic` and `fetch_update`.
- **Replacing raw pointers with Address**: In types like test slots, replacing `*mut Atomic<T>` with `Address` eliminates the need for `unsafe impl Send` while preserving functionality via `to_mut_ptr()`.
- **Refactoring to use slot_for for Metadata**: Using `slot_for(...).load()` and `slot_for(...).store(...)` on `SideMetadataSpec` can eliminate unsafe blocks for direct metadata access in production code.
- **Replacing Address::zero()**: Replaced `unsafe { Address::zero() }` with `Address::ZERO` in `malloc_ms/global.rs`.
- **Safe is_marked**: Replaced `is_marked_unsafe` with `is_marked(..., Ordering::Relaxed)` in `malloc_ms/global.rs` to eliminate unsafe blocks.
- **Safe unset_mark_bit**: Made `unset_mark_bit` safe in `metadata.rs` by using `store_atomic` and updated call sites.
- **Safe unset_vo_bit**: Used safe `unset_vo_bit` instead of `unset_vo_bit_unsafe` in `malloc_ms/global.rs`.
- **Safe unset_page_mark**: Refactored `unset_page_mark` in `malloc_ms/global.rs` to use safe `is_page_marked` and `unset_page_mark` (using atomics) from `metadata.rs`, removing unsafe from the function and its call sites.
- **Centralizing unsafe stores in loops**: In `block.rs`, centralized unsafe raw memory writes in `simple_sweep` and `naive_brute_force_sweep` into a safe helper `write_free_list_link` with `debug_assert!`.
- **Passing mutable reference instead of calling mut_self**: In `map32.rs`, refactored `free_contiguous_chunks_no_lock` to take `&mut Map32Inner` to eliminate unsafe blocks calling `self.mut_self()`.
- **Using MetadataSlot in vo_bit**: Used `VO_BIT_SIDE_METADATA_SPEC.slot_for` to eliminate unsafe blocks in `vo_bit/mod.rs`.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/util/copy/mod.rs` — All unsafe removed.
- `src/policy/sft_map.rs` — Remaining unsafe are trait signatures and unavoidable transmutes for fat pointers in atomics.
- `src/util/alloc/allocators.rs` — Remaining unsafe are getters using `assume_init_ref/mut` on `MaybeUninit` arrays, required for layout compatibility.
- `src/util/metadata/side_metadata/helpers.rs` — Remaining unsafe are raw loads in functions that scan metadata addresses directly without a spec.
- `src/util/metadata/side_metadata/global.rs` — Remaining unsafe are `new_unchecked` on raw metadata addresses, `std::ptr::copy`, and `unsafe fn` due to concurrency invariants. (Analyzed in step bf97f4bf)
- `src/util/metadata/metadata_val_traits.rs` — `MetadataValue` trait methods are unsafe by design as they perform raw memory access.
- `src/util/metadata/side_metadata/side_metadata_tests.rs` — All unsafe are `Address::from_usize(...)` for creating test addresses.
- `src/policy/marksweepspace/native_ms/block.rs` — Remaining unsafe are `Address::from_usize`, `ObjectReference::from_raw_address_unchecked`, and raw pointer dereferencing for `BlockList`.
- `src/util/heap/layout/map32.rs` — Remaining unsafe are `mut_self` calls claimed to be safe due to single-threaded boot time or exclusive ranges.
- `src/plan/mutator_context.rs` — Remaining unsafe are allocator accesses relying on `Allocators` unsafe methods, required for layout compatibility.
- `src/util/heap/blockpageresource.rs` — Custom lock-free queue (`BlockQueue`) using `UnsafeCell` and `MaybeUninit`.

## Abstraction Proposals (for Phase 2)
- **Safe Metadata Accessor**: `MetadataSlot` implemented in `metadata_val_traits.rs`. Used in `header_metadata.rs` and `global.rs`.
