# Step Analysis (auto-saved)

## Target
- File: `src/policy/sft_map.rs`, `src/policy/copyspace.rs`, `src/util/rust_util/atomic_box.rs`
- Strategy: Investigate feasibility of reducing unsafe code and confirm irreducibility.

## Findings
- `src/policy/sft_map.rs`: The use of `transmute` to store and load fat pointers (`&dyn SFT`) atomically as `DoubleWord` (or `usize` equivalent) is irreducible. Rust does not support atomic operations on fat pointers directly. Casting them to integers and back requires `transmute` or raw pointer casts, both of which are unsafe. Any safe abstraction would just move the unsafe inside it without improving safety.
- `src/policy/copyspace.rs`: The lifetime extension in `rebind` (line 367) using `unsafe { &*{ space as *const _ } }` is irreducible without a massive refactor. The `Allocator` trait and `BumpAllocator` struct require `'static` references to spaces. Since `CopySpace` is passed by reference with a local lifetime in `prepare_worker`, unsafe is needed to bypass the borrow checker.
- `src/util/rust_util/atomic_box.rs`: The lock-free `OnceOptionBox` requires raw pointer dereferences and `Box::from_raw` in `get` and `get_or_init`. Replacing it with `OnceLock` would add a check on every read (unless optimized away, which isn't guaranteed) and was rejected by previous agents to maintain zero-cost reads and low overhead for large arrays (e.g., in `two_level_storage.rs`).

## Attempted Changes
- Analyzed code and usages. Determined that these cases are genuinely irreducible or require massive architectural changes that are out of scope for local removals.

## Blockers / Insights for Next Step
- All remaining unsafe blocks in the investigated files are deemed irreducible.
- I will update `UNSAFE_MEMORY.md` to reflect these findings and move these files to the "Files NOT to Revisit" section with "Phase 2 confirmed".
