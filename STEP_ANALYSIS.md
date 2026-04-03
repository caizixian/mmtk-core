# Step Analysis (auto-saved)

## Target
- File: `src/util/metadata/side_metadata/global.rs`
- Strategy: Investigate making `MetadataSlot` methods safe by moving unsafe to constructor or checking validity.

## Findings
- `MetadataSlot` is a wrapper around `Address`.
- Its methods (like `load`, `store`) are currently safe but contain `unsafe` blocks because they perform pointer dereferences.
- Moving the `unsafe` block to the constructor (making it `unsafe fn new`) would require adding `unsafe` blocks at all call sites.
- There are at least 18 call sites in `global.rs` and many more in `header_metadata.rs`, `free_list_allocator.rs`, etc.
- Moving `unsafe` to the constructor would likely increase the total count of `unsafe` blocks in the codebase.
- Checking validity of the address at runtime is not feasible without a performance penalty in the GC hot path.
- Therefore, the current implementation (safe methods with unsafe inside) is the most count-efficient way to encapsulate this unsafety, even if it relies on caller invariants.

## Attempted Changes
- None (analysis only).

## Blockers / Insights for Next Step
- The Work Queue item is likely a dead end in terms of reducing the count of unsafe blocks.
- Most files with unsafe have been marked as irreducible in `UNSAFE_MEMORY.md`.
- We may be approaching a steady state where remaining unsafe is irreducible.
