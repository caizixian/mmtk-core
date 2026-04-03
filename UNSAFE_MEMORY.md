# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 722 | Current: 633 | Δ: -89
- Completed subsystems: util/alloc (partial), policy/sft_map (partial), policy/sft_map (partial), policy/marksweepspace (block.rs cleanup), util/heap/layout (vm_layout.rs static mut, map32.rs Mutex, map64.rs RwLock), util/copy (MaybeUninit to Option in GCWorkerCopyContext), util/metadata/side_metadata (side_metadata_tests.rs Address::from_usize(0) cleanup, global.rs SideMetadataOffset union to enum), util/heap/gc_trigger.rs (OnceLock for plan), util/metadata/header_metadata.rs (TestBuffer yield slice for tests)

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
- `MaybeUninit::uninit().assume_init()` for arrays → `[const { MaybeUninit::uninit() }; N]` — works when type is `MaybeUninit`.
- `get_unchecked(i)` → `[i]` — works in non-hot paths where bounds are guaranteed or panic is acceptable.
- `*const T` → `&T` in trait signatures where ownership is not required and lifetimes are valid.
- `unsafe impl Sync` removal for types that only contain thread-safe fields (auto-Sync).
- `static mut` → `OnceLock` for write-once globals.
- `MaybeUninit` → `OnceLock` for late-initialized fields (e.g. `GCTrigger::plan`).
- Raw memory allocation in tests → `TestBuffer<T>` safe wrapper with `Drop` for automatic cleanup.

## Refactoring Ideas
- `src/util/metadata/side_metadata/helpers.rs`: Analyze remaining unsafe blocks (mostly tests/Address::from_usize and load/store).
- `src/util/metadata/metadata_val_traits.rs`: Analyze 20 unsafe blocks (mostly trait methods for load/store).
- `src/util/metadata/side_metadata/global.rs`: Analyze 89 unsafe blocks (likely many load/store or FFI).

## Files NOT to Revisit
- `src/util/memory.rs` — FFI calls to libc (mmap, munmap, etc.).
- `src/util/heap/layout/map32.rs` — Remaining unsafe is SFT_MAP.clear (side metadata access).
- `src/util/heap/layout/map64.rs` — Remaining unsafe are trait methods or `Address::from_usize` (anti-pattern to replace with `ZERO.add`).
