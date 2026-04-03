# Step Analysis (auto-saved)

## Target
- File: Holistic review of `raw_memory_freelist.rs`, `malloc_ms/global.rs`, `allocator.rs`, and `sft_map.rs`.
- Strategy: Verify if any remaining unsafe can be reduced or abstracted.

## Findings
- `src/util/raw_memory_freelist.rs`: Line 146 creates a `'static` mutable slice from raw memory. This is irreducible for count reduction because replacing it with pointer arithmetic would require unsafe blocks in `get_entry` and `set_entry`, increasing the total count.
- `src/policy/marksweepspace/malloc_ms/global.rs`: Line 555 casts `&mut self` to `&'static Self` to pass to work packets. This is a standard MMTk pattern to bypass the borrow checker for concurrent GC work and is irreducible without major architectural changes.
- `src/util/alloc/allocator.rs`: Line 184 uses `std::ptr::write_bytes` to fill memory. This is the primitive operation; creating a slice to use `fill` would still require unsafe and not reduce the count.
- `src/policy/sft_map.rs`: Line 111 and 138 involve lifetime extension and raw pointer dereference. These are needed to return `&'static dyn SFT` lock-free on the hot path (object tracing) without the overhead of `Arc`.

## Attempted Changes
- None. Confirmed that remaining unsafe is irreducible or well-encapsulated.

## Blockers / Insights for Next Step
- The project remains in a steady state for Phase 3 (Irreducible Documentation). All files with unsafe have been audited and added to "Files NOT to Revisit". No further reductions are possible with current strategies.
