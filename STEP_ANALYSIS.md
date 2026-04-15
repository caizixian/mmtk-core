# Step Analysis (auto-saved)

## Target
- File: Holistic review of all remaining unsafe locations
- Strategy: Verify irreducibility and encapsulation status due to strategy escalation

## Findings
- `src/util/memory.rs` (9 unsafe): Irreducible FFI calls to libc memory management (`mmap`, `munmap`, `mprotect`, `madvise`).
- `src/util/malloc/malloc_ms_util.rs` (7 unsafe) & `mod.rs` (4 unsafe): Irreducible FFI calls to standard allocator functions (`malloc`, `free`, `calloc`, `posix_memalign`).
- `src/util/address.rs` (7 unsafe): Low-level primitives for raw memory operations (`load`, `store`, etc.). These are the building blocks for other safe abstractions and cannot be made safe themselves without safe pointers (which is what they are abstracting).
- `src/util/metadata/side_metadata/helpers.rs` (6 unsafe): Implementation of `MetadataCursor` abstraction. The unsafe blocks here are necessary to encapsulate the unsafe operations on `Address`, providing a safe API to the rest of the codebase.
- `src/vm/slot.rs` (3 unsafe): Raw pointer dereferences in `SimpleSlot` (`load`/`store`) and `MemorySlice::copy` (`std::ptr::copy`) are necessary for GC heap operations where we operate on addresses rather than Rust references.
- `src/util/rust_util/mod.rs` (4 unsafe): `ProofCell` uses `UnsafeCell` and requires unsafe blocks for zero-cost reads. It uses proof tokens for safe writes. This is a deliberate design choice for performance on hot paths.
- `src/util/alloc/allocators.rs` (2 unsafe): `MaybeUninit` is used for FFI layout compatibility in the `Allocators` struct, which is `#[repr(C)]` and exposed to VM bindings.
- `src/util/test_util/fixtures.rs` (2 unsafe): Unsafe pointer casts (`*const` to `*mut`) in tests are used to handle `'static` lifetime requirements imposed by the GC architecture (e.g., `bind_mutator`).
- `docs/dummyvm/src/api.rs` (4 unsafe): Irreducible FFI boundary operations (e.g., `Box::from_raw`) to handle data passed from the VM.

## Attempted Changes
- Analyzed all files with remaining unsafe code.
- Confirmed that all remaining unsafe code is either:
  1. Irreducible FFI calls.
  2. Low-level memory primitives that are necessary for GC implementation.
  3. Properly encapsulated inside safe abstractions (like `MetadataCursor` and `ProofCell`).
  4. Required for FFI layout compatibility.
  5. Test-only workarounds for lifetime requirements.

## Blockers / Insights for Next Step
- Concluded that the remaining 85 unsafe items are genuinely irreducible or properly encapsulated as per the current architecture of `mmtk-core`.
- No further reductions are possible without major architectural changes (e.g., moving away from `Address` to references, which is impractical for a GC, or changing the FFI boundary).
