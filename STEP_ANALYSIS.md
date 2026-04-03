# Step Analysis (auto-saved)

## Target
- File: `src/policy/marksweepspace/native_ms/block.rs`
- Strategy: Investigate `BlockList` context pattern to remove unsafe stores.

## Findings
- **`src/policy/marksweepspace/native_ms/block.rs`**:
  - Line 208: `unsafe { cell.store::<Address>(next); }` in `write_free_list_link`. This is a raw memory write. It is a centralized helper. Callers are safe or rely on GC invariants (world stopped).
  - The `BlockList` pattern suggested in the prompt (passing context) would require changing trait signatures or adding overhead. Given the performance-critical nature of sweeping, and that the unsafe is already centralized in a helper, this is considered irreducible for now.
  - Line 287 & 327: `ObjectReference::from_raw_address_unchecked` is used to create an object reference to check the mark bit. This is necessary because `is_marked` takes `ObjectReference` and might read the header. This is considered irreducible unless `is_marked` can take an `Address`.

- **`src/util/alloc/allocators.rs`**:
  - Uses `[MaybeUninit<T>; N]` arrays for allocators.
  - The comment states: "We are trying to make it fixed-sized so that VM bindings can easily define a Mutator type to have the exact same layout as our Mutator struct."
  - Replacing with `Option` might change the layout and break ABI compatibility with VM bindings.
  - The unsafe `assume_init_ref/mut` calls are centralized in `get_allocator` methods and guarded by a bitmap check. Considered irreducible due to ABI constraints.

- **`src/util/rust_util/mod.rs` (`InitializeOnce`)**:
  - Used for `SFT_MAP` to provide zero-overhead reads in release builds.
  - Provides `get_mut` from `&self` which is used during plan creation (single-threaded).
  - `OnceLock` does not provide `get_mut` from `&self`, and adding a check on every read might affect performance of `SFT_MAP` lookups.
  - Considered irreducible due to performance constraints and specific API requirements.

- **`src/util/conversions.rs`**:
  - The listed unsafe blocks for `transmute` in `raw_align_up/down` do not exist in the current version (they are safe arithmetic).
  - The only unsafe in production code is `Address::from_usize` in `chunk_index_to_address`, which is inherently unsafe by design.

## Attempted Changes
- None. Analyzed the targets and concluded that the remaining unsafe are likely irreducible or already centralized according to the design constraints of MMTk (performance, ABI compatibility with VM bindings).

## Blockers / Insights for Next Step
- The project is in Phase 2, but many of the remaining unsafe blocks are in low-level primitives or performance-critical paths where safe alternatives (like `OnceLock` or `Option`) might introduce overhead or break layout guarantees.
- I should report this to the user or seek further guidance if we want to push for safety at the cost of performance or ABI compatibility.
