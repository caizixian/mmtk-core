# Step Analysis (auto-saved)

## Target
- File: docs/dummyvm/src/api.rs
- Strategy: Refactor FFI functions to use `Option<&mut T>` instead of `*mut T` where only a reference is needed, eliminating unsafe blocks for dereferencing.

## Findings
- Line 32: `let builder = unsafe { &mut *builder };` — Can be replaced by `Option<&mut MMTKBuilder>` in signature.
- Line 43: `let builder = unsafe { &mut *builder };` — Can be replaced by `Option<&mut MMTKBuilder>` in signature.
- Line 101: `unsafe { &mut *mutator }` — Can be replaced by `Option<&mut Mutator<DummyVM>>` in signature.
- Line 123: `unsafe { &mut *mutator }` — Can be replaced by `Option<&mut Mutator<DummyVM>>` in signature.

## Attempted Changes
- Refactored `mmtk_set_option_from_string`, `mmtk_set_fixed_heap_size`, `mmtk_alloc`, and `mmtk_post_alloc` in `docs/dummyvm/src/api.rs` to use `Option<&mut T>` instead of `*mut T`.
- Updated test calls in `api.rs` to use `unsafe { ptr.as_mut() }`.

## Blockers / Insights for Next Step
- The tests passed successfully, confirming that `Option<&mut T>` is FFI-compatible and works in this context.
- This reduced unsafe blocks in prod by 4 (or at least moved them to test).

