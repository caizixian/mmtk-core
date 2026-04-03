# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 722 | Current: 519 | Δ: -203
- Completed subsystems: util/alloc/allocator.rs, util/alloc/free_list_allocator.rs, policy/marksweepspace (and native_ms/block.rs safe load_atomic), util/heap/layout/map32.rs (removed unnecessary unsafe block), util/heap/layout, util/copy, util/metadata/side_metadata, util/heap/gc_trigger.rs, util/metadata/header_metadata.rs, util/heap/blockpageresource.rs, scheduler/gc_work.rs, util/metadata/vo_bit, util/linear_scan, vm/tests/mock_tests/mock_test_slots.rs, util/heap/freelistpageresource.rs, util/rust_util, policy/sft_map (SFTMap update/eager_initialize take reference, remove unsafe in implementations), policy/marksweepspace/malloc_ms/global.rs (unnecessary SFT_MAP unsafe, safe page marks), policy/lockfreeimmortalspace.rs (unnecessary eager_initialize unsafe), mmtk.rs (removed unnecessary unsafe cast for GCTrigger mutation), policy/marksweepspace/malloc_ms/metadata.rs (safe page marks), util/metadata/side_metadata/global.rs (combined unsafe blocks in bcopy_metadata_contiguous), util/heap/monotonepageresource.rs (made reset/release_pages safe), policy/copyspace.rs (removed unnecessary unsafe)


## Codebase Invariants (PROTECTED — do not prune)
- "Work packets are single-use — Option::take() is safe for extracting owned data"
- "Address arithmetic is safe; only load/store/deref requires unsafe"
- "SFT_MAP is initialized once during single-threaded startup, then read-only"

## Remaining Unsafe by Subsystem
### util/alloc (src/util/alloc/allocators.rs)
- ~4 unsafe — get_allocator fast paths (UncheckedCall/RawPointerDeref)
- Why: Assumes allocator is initialized for performance.

### policy/sft_map (src/policy/sft_map.rs)
- ~17 unsafe — transmute for fat pointers, side metadata store_atomic.
- Why: FFI/Performance hot paths (transmute for fat pointers, side metadata access). `get_unchecked` is now safe as implementations use standard indexing.

## Patterns Discovered
- `union` to `enum` for types that can be either A or B (e.g. `SideMetadataOffset`). Eliminates `unsafe` for field access and allows safe `PartialEq`/`Eq`/`Hash` derivation.
- `UnsafeCell<T>` → `RwLock<T>` (or `Mutex<T>`) for thread-safe interior mutability in shared data structures (e.g. `Map64`).
- `UnsafeCell::get_mut()` / `AtomicXxx::get_mut()` when uniqueness is guaranteed (e.g. local variables or `&mut self` methods) to bypass `unsafe` and atomic operations entirely.
- `MaybeUninit::uninit().assume_init()` for arrays → `[const { MaybeUninit::uninit() }; N]` — works when type is `MaybeUninit`.
- `get_unchecked(i)` → `[i]` — works in non-hot paths where bounds are guaranteed or panic is acceptable.
- `unsafe fn` in trait → safe `fn` when all implementations are safe (e.g. use standard indexing instead of raw pointers). Removes `unsafe` blocks at call sites.
- `SFTRawPointer` (raw pointer) to `&(dyn SFT + Sync + 'static)` (reference) in trait signatures for `update` and `eager_initialize`. Removes `unsafe` blocks when dereferencing the space object in implementations.
- `*const T` → `&T` in trait signatures where ownership is not required and lifetimes are valid.
- `unsafe impl Sync` removal for types that only contain thread-safe fields (auto-Sync).
- `InitializeOnce<T>` bound tightening: `unsafe impl<T: Sync> Sync for InitializeOnce<T>` to prevent unsound sharing of non-Sync types.
- `SFTMap: Sync` trait bound to enforce thread-safety for SFT implementations.
- `static mut` → `OnceLock` for write-once globals.
- `MaybeUninit` → `OnceLock` for late-initialized fields (e.g. `GCTrigger::plan`).
- Raw memory allocation in tests → `TestBuffer<T>` safe wrapper with `Drop` for automatic cleanup.
- `from_raw_address_unchecked` → `from_raw_address().unwrap()` to replace UB with panic.
- `SideMetadataSpec::load` → `load_atomic` with `Ordering::Relaxed` for safe side metadata reads where single-threaded or relaxed consistency is sufficient.
- `SideMetadataSpec::store` → `store_atomic` with `Ordering::SeqCst` (or `Relaxed` if safe) for safe side metadata writes.
- `*mut T` → `&'a T` in test fixtures/mock types where lifetimes can be tracked, eliminating unsafe raw pointer dereferences (requires manual PartialEq/Eq/Hash for pointer equality).
- `as_ref::<AtomicU8>()` → `MetadataValue::fetch_and` / `fetch_or` / `load_atomic` / `store_atomic` — use standard trait abstractions instead of raw pointer casts.
- `*mut T = val` in tests → `MetadataValue::store(meta_addr, val)` — use abstractions instead of raw pointers in tests.

## Refactoring Ideas
- Scan for other usages of non-atomic SideMetadataSpec::load that can be replaced with load_atomic(Ordering::Relaxed) to remove unsafe blocks.
- Investigate `Mutator::allocator_impl_mut_for_semantic` in `src/plan/mutator_context.rs` to see if downcasting of raw trait objects can be made safer or abstracted.

## Files NOT to Revisit
- `src/util/memory.rs` — FFI calls to libc (mmap, munmap, etc.).
- src/util/heap/layout/map32.rs — Completed (no remaining unsafe blocks, only trait-required unsafe fn).
- `src/util/heap/layout/map64.rs` — Remaining unsafe are trait methods or `Address::from_usize` (anti-pattern to replace with `ZERO.add`).
- `src/util/heap/freelistpageresource.rs`: Remove `unsafe impl Send` and `unsafe impl Sync` for `FreeListPageResource` now that `FreeList` trait is `Send`.
- `src/util/heap/space_descriptor.rs` — `Zeroable` implementation is required for `new_zeroed_vec` in `map32.rs`. Remaining `unsafe` are `Address::from_usize` which are anti-patterns to replace.
- `src/util/metadata/side_metadata/helpers.rs` — Remaining `unsafe` are reading raw bytes from side metadata memory (irreducible `RawHeapAccess`) or `Address::from_usize`.

