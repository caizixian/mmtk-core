# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 722 | Current: 708 | Δ: -14
- Completed subsystems: util/alloc (partial), policy/sft_map (partial), policy/marksweepspace (block.rs cleanup)

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
- `MaybeUninit::uninit().assume_init()` for arrays → `[const { MaybeUninit::uninit() }; N]` — works when type is `MaybeUninit`.
- `get_unchecked(i)` → `[i]` — works in non-hot paths where bounds are guaranteed or panic is acceptable.
- `*const T` → `&T` in trait signatures where ownership is not required and lifetimes are valid.
- `unsafe impl Sync` removal for types that only contain thread-safe fields (auto-Sync).

## Refactoring Ideas
- `src/policy/marksweepspace/native_ms/block.rs`: Analyzed. Remaining 22 are mostly side metadata access (RawHeapAccess).
- `src/util/metadata/side_metadata/helpers.rs`: Analyze 34 unsafe blocks for potential safety improvements.

## Files NOT to Revisit
- `src/util/memory.rs` — FFI calls to libc (mmap, munmap, etc.).
