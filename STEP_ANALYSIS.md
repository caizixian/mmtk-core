# Step Analysis (auto-saved)

## Target
- File: `src/util/malloc/malloc_ms_util.rs`
- Strategy: Check if we can use `Layout` and `alloc` instead of `posix_memalign` to avoid FFI.

## Findings
- Line 11: `let result = unsafe { posix_memalign(ptr_ptr, align, size) };`
- This is an FFI call to the selected allocator library (libc, jemalloc, or mimalloc).
- Replacing this with `std::alloc::alloc` would use the Rust global allocator, which may not be the one intended by the MMTk features (e.g., if `malloc_jemalloc` is enabled but Rust uses system allocator).
- Furthermore, `std::alloc::dealloc` requires the `Layout` (size and alignment) to be provided at deallocation. The current implementation of `offset_free` relies on `libc::free` which only needs the pointer. Storing the layout would introduce overhead.
- Therefore, this FFI call is considered irreducible to maintain the design of using specific allocator libraries and avoiding overhead.

## Attempted Changes
- Analyzed the feasibility of replacing `posix_memalign` with `std::alloc::alloc`.

## Blockers / Insights for Next Step
- Confirmed that remaining unsafe in `malloc_ms_util.rs` is irreducible.
- The task seems to have reached a point where most remaining unsafe is irreducible or by design (FFI, core primitives).
- Next step should focus on documenting this conclusion and updating the work queue to reflect completion or next steps in documentation.
