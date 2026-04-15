# Step Analysis (auto-saved)

## Target
- File: src/util/test_util/mock_vm.rs, src/util/test_util/fixtures.rs
- Strategy: Local removal analysis

## Findings
- `src/util/test_util/mock_vm.rs`: Uses `transmute` in `lifetime!` macro to remove lifetimes for mock methods. This is a workaround for the mock framework's inability to handle generic lifetimes. Irreducible without a major redesign of the mock framework.
- `src/util/test_util/fixtures.rs`: Uses `Box::from_raw` in `Drop` to clean up a leaked `MMTK` instance. The leakage is required because `initialize_collection` requires a `'static` reference. Irreducible without changing the lifetime requirements of the core initialization API.
- Also analyzed `space.rs`, `immortalspace.rs`, `largeobjectspace.rs`, `forwarding.rs`, `bumpallocator.rs`, `work_counter.rs`, `library.rs`, `opaque_pointer.rs` and found them to be clean of `unsafe` blocks or only containing safe wrappers.

## Attempted Changes
- None. Analysis showed remaining unsafe is likely irreducible or requires large-scale refactoring not suitable for local removals.

## Blockers / Insights for Next Step
- Most files with unsafe code are already in "Files NOT to Revisit".
- The remaining files with unsafe code (like test utilities) have irreducible unsafe due to framework designs.
- Need to look for new abstraction opportunities or accept that we are near the limit of unsafe reduction without changing core designs.
