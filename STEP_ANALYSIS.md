# Step Analysis (auto-saved)

## Target
- File: `src/util/metadata/side_metadata/global.rs`
- Strategy: Refactor `MetadataSlot` to use references or `AtomicPtr` to reduce unsafe.

## Findings
- `MetadataSlot` contains about 20 methods with 1 unsafe block each.
- These methods are safe wrappers around raw memory operations on `Address`.
- There are about 17 call sites that construct `MetadataSlot(addr)` on the fly.
- If we refactor `MetadataSlot` to hold a reference (e.g. `&AtomicU8`), we would make the methods safe (removing ~7-20 unsafe blocks).
- However, we would need to add an `unsafe` block at each of the 17 creation sites to create the reference from the address.
- Even with combining some creation sites in the same scope, the total number of `unsafe` blocks in the source code would likely INCREASE or stay the same.
- Therefore, this refactor does not help reduce the count of `unsafe` blocks and might make the code more verbose.

## Attempted Changes
- None (concluded it's an anti-pattern for count reduction).

## Blockers / Insights for Next Step
- Most files with significant unsafe are marked as "Files NOT to Revisit".
- Need to find files with 1 or 2 unsafe blocks that are NOT in the "Files NOT to Revisit" list.
- These files are not listed in the prompt's "Exact Unsafe Locations" section because it only lists top files.
- Added `src/policy/copyspace.rs` to the work queue for re-evaluation in the next step.
