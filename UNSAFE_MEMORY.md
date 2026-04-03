# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 722 | Current: 593 | Δ: -129
- Completed subsystems: util/alloc/allocator.rs, util/alloc/free_list_allocator.rs, policy/marksweepspace, util/heap/layout, util/copy, util/metadata/side_metadata (side_metadata_tests.rs load/store to load_atomic/store_atomic), util/heap/gc_trigger.rs, util/metadata/header_metadata.rs, util/heap/blockpageresource.rs, scheduler/gc_work.rs, util/metadata/vo_bit, util/linear_scan, vm/tests/mock_tests/mock_test_slots.rs


## Codebase Invariants (PROTECTED — do not prune)
- "Work packets are single-use — Option::take() is safe for extracting owned data"
- "Address arithmetic is safe; only load/store/deref requires unsafe"
- "SFT_MAP is initialized once during single-threaded startup, then read-only"

## Remaining Unsafe by Subsystem
### util/alloc (src/util/alloc/allocators.rs)
- ~4 unsafe — get_allocator fast paths (UncheckedCall/RawPointerDeref)
- Why: Assumes allocator is initialized for performance.

### policy/sft_map (src/policy/sft_map.rs)
- ~21 unsafe — SFT map access and trait impl.
- Why: FFI/Performance hot paths (transmute for fat pointers, slice access).

## Patterns Discovered
- `union` to `enum` for types that can be either A or B (e.g. `SideMetadataOffset`). Eliminates `unsafe` for field access and allows safe `PartialEq`/`Eq`/`Hash` derivation.
- `UnsafeCell<T>` → `RwLock<T>` (or `Mutex<T>`) for thread-safe interior mutability in shared data structures (e.g. `Map64`).
- `UnsafeCell::get_mut()` / `AtomicXxx::get_mut()` when uniqueness is guaranteed (e.g. local variables or `&mut self` methods) to bypass `unsafe` and atomic operations entirely.
- `MaybeUninit::uninit().assume_init()` for arrays → `[const { MaybeUninit::uninit() }; N]` — works when type is `MaybeUninit`.
- `get_unchecked(i)` → `[i]` — works in non-hot paths where bounds are guaranteed or panic is acceptable.
- `*const T` → `&T` in trait signatures where ownership is not required and lifetimes are valid.
- `unsafe impl Sync` removal for types that only contain thread-safe fields (auto-Sync).
- `static mut` → `OnceLock` for write-once globals.
- `MaybeUninit` → `OnceLock` for late-initialized fields (e.g. `GCTrigger::plan`).
- Raw memory allocation in tests → `TestBuffer<T>` safe wrapper with `Drop` for automatic cleanup.
- `from_raw_address_unchecked` → `from_raw_address().unwrap()` to replace UB with panic.
- `SideMetadataSpec::load` → `load_atomic` with `Ordering::Relaxed` for safe side metadata reads where single-threaded or relaxed consistency is sufficient.
- `*mut T` → `&'a T` in test fixtures/mock types where lifetimes can be tracked, eliminating unsafe raw pointer dereferences (requires manual PartialEq/Eq/Hash for pointer equality).
- `SimpleSlot` → `&Atomic<T>` in tests for direct access without raw pointers.

## Refactoring Ideas
- `src/util/metadata/side_metadata/helpers.rs`: Analyze remaining unsafe blocks (mostly tests/Address::from_usize and load/store).
- `src/util/metadata/metadata_val_traits.rs`: Analyze 20 unsafe blocks (mostly trait methods for load/store).
- `src/util/metadata/side_metadata/global.rs`: Analyze 89 unsafe blocks (likely many load/store or FFI).
- `src/policy/sft_map.rs`: Analyze remaining unsafe (20 count) for potential safe abstractions in SFT map access.

## Files NOT to Revisit
- `src/util/memory.rs` — FFI calls to libc (mmap, munmap, etc.).
- `src/util/heap/layout/map32.rs` — Remaining unsafe is SFT_MAP.clear (side metadata access).
- `src/util/heap/layout/map64.rs` — Remaining unsafe are trait methods or `Address::from_usize` (anti-pattern to replace with `ZERO.add`).
