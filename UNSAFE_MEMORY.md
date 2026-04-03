# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 722 | Current: 714 | Δ: -8
- Completed subsystems: util/alloc (partial), policy/sft_map (partial)

## Codebase Invariants (PROTECTED — do not prune)
- "Work packets are single-use — Option::take() is safe for extracting owned data"
- "Address arithmetic is safe; only load/store/deref requires unsafe"
- "SFT_MAP is initialized once during single-threaded startup, then read-only"

## Remaining Unsafe by Subsystem
### util/alloc (src/util/alloc/allocators.rs)
- ~4 unsafe — get_allocator fast paths (UncheckedCall/RawPointerDeref)
- Why: Assumes allocator is initialized for performance.

### policy/sft_map (src/policy/sft_map.rs)
- ~24 unsafe — SFT map access and trait impl.
- Why: FFI/UnsafeTraitImpl/Performance hot paths.

## Patterns Discovered
- `MaybeUninit::uninit().assume_init()` for arrays → `[const { MaybeUninit::uninit() }; N]` — works when type is `MaybeUninit`.
- `get_unchecked(i)` → `[i]` — works in non-hot paths where bounds are guaranteed or panic is acceptable.
- `*const T` → `&T` in trait signatures where ownership is not required and lifetimes are valid.

## Refactoring Ideas
- `src/policy/sft_map.rs`: Check if `unsafe impl Sync` can be removed or justified.
- `src/util/address.rs`: Verify if any `SFT_MAP` access can be made safe using a safe getter if available.

## Files NOT to Revisit
- `src/util/memory.rs` — FFI calls to libc (mmap, munmap, etc.).
