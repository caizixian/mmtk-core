# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 722 | Current: 716 | Δ: -6
- Completed subsystems: util/alloc (partial)

## Codebase Invariants (PROTECTED — do not prune)
- "Work packets are single-use — Option::take() is safe for extracting owned data"
- "Address arithmetic is safe; only load/store/deref requires unsafe"
- "SFT_MAP is initialized once during single-threaded startup, then read-only"

## Remaining Unsafe by Subsystem
### util/alloc (src/util/alloc/allocators.rs)
- ~4 unsafe — get_allocator fast paths (UncheckedCall/RawPointerDeref)
- Why: Assumes allocator is initialized for performance.

### policy/sft_map (src/policy/sft_map.rs)
- ~26 unsafe — SFT map access and trait impl.
- Why: FFI/UnsafeTraitImpl.

## Patterns Discovered
- `MaybeUninit::uninit().assume_init()` for arrays → `[const { MaybeUninit::uninit() }; N]` — works when type is `MaybeUninit`.

## Refactoring Ideas
- `src/policy/sft_map.rs`: Check if `unsafe impl Sync` can be removed or justified.
- `src/util/address.rs`: Verify if any `SFT_MAP` access can be made safe using a safe getter if available.

## Files NOT to Revisit
- `src/util/memory.rs` — FFI calls to libc (mmap, munmap, etc.).
