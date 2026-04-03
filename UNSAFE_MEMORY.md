# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 351 | Current: 242 | Δ: -109
- Phase: 2

## Codebase Invariants (PROTECTED — do not prune)
Architectural insights that affect ALL future safety decisions:
- Work packets hold raw pointers to plans or spaces to bypass borrow checker and lifetimes.
- Static plan references are needed because plan types are generic and cannot be stored in global statics easily.
- `BlockQueue` in `BlockPageResource` was refactored to use `ArrayQueue` and `Mutex` for thread-local queues, eliminating custom lock-free code and associated unsafe blocks.

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🔴 HIGH: `src/scheduler/worker.rs:304` — Investigate if `WorkerGroup` can eliminate `unsafe impl Sync` — expected Δ: 1

## Patterns Discovered
Reusable refactoring patterns (recipe format):
- Adding `Send + Sync` bounds to trait objects (e.g. `Box<dyn Trait + Send + Sync>`) can enable automatic derivation of `Send` and `Sync` for containing structures, eliminating the need for `unsafe impl Send` or `Sync`.
- Introducing `StwProof` token to encapsulate raw memory operations or mutable access to shared structures (like `Plan`) during Stop-The-World phases.
- Introducing `MetadataSlot` abstraction to encapsulate raw memory operations on metadata addresses behind a safe API.
- Safe wrappers in `Mutator` (like `get_allocator_mut_safe`) can encapsulate `unsafe` array access by checking initialization against `space_mapping`.
- Extending `MetadataSlot` with generic methods for `MetadataValue` allows centralizing unsafe operations on types larger than `u8`.
- Using safe wrappers in `Mutator` (like `allocator_impl_mut_for_semantic`) in plan-specific mutators to eliminate direct unsafe calls to `allocators.get_allocator_mut`.
- Replacing platform-specific FFI calls with safe standard library equivalents (e.g. `std::thread::available_parallelism` instead of `sched_getaffinity`).
- Replacing `ObjectReference::from_raw_address_unchecked` with `ObjectReference::from_raw_address(...).unwrap()` when the address is guaranteed to be non-zero (e.g. checked by assertion or invariant).
- Changing trait methods to take references instead of raw pointers when call sites already have references, eliminating unsafe blocks used for dereferencing or coercion.
- Centralizing thread-safety guarantees in low-level utilities (like `BlockQueue` or `BlockPageResource`) can eliminate `unsafe impl Sync` in high-level types (like spaces) that use them.
- Replacing custom lock-free queues with `crossbeam::queue::ArrayQueue` and using `Mutex` for thread-local access can eliminate unsafe code in queue implementations.
- Using Generic Associated Types (GATs) in `ObjectTracerContext` to allow `TracerType` to borrow `GCWorker` without raw pointers.
- Using `Box::leak` in test fixtures to avoid raw pointer management and lifetimes, making the fixture safe.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/plan/concurrent/concurrent_marking_work.rs` — Eliminated raw pointer from `ConcurrentTraceObjects` by using a temporary tracer type. Localized unsafe in the tracer. [Phase 2 confirmed]
- `src/util/metadata/metadata_val_traits.rs` — Irreducible raw loads from addresses in trait default impls. [Phase 2 confirmed]
- `src/util/memory.rs` — Contains wrappers for FFI calls. [Phase 2 confirmed]
- `docs/dummyvm/src/api.rs` — Irreducible FFI boundaries in dummy VM. [Phase 2 confirmed]
- `src/util/malloc/malloc_ms_util.rs` — Irreducible FFI and raw pointer manipulation. [Phase 2 confirmed]
- `src/util/address.rs` — Primitives for address operations. [Phase 2 confirmed]
- `src/util/metadata/side_metadata/side_metadata_tests.rs` — Tests use unsafe to check address iteration. [Phase 2 confirmed]
- `src/util/rust_util/atomic_box.rs` — Custom lock-free lazily initialized box. [Phase 2 confirmed]
- `src/util/rust_util/mod.rs` — `InitializeOnce` is irreducible for performance. [Phase 2 confirmed]
- `src/policy/marksweepspace/native_ms/block.rs` — Raw memory accesses for free list. [Phase 2 confirmed]
- `src/util/metadata/side_metadata/global.rs` — Remaining unsafe are irreducible function signatures and raw memory copy. [Phase 2 confirmed]
- `src/policy/marksweepspace/malloc_ms/global.rs` — Remaining unsafe are irreducible FFI and lifetime extension. [Phase 2 confirmed]
- `src/policy/copyspace.rs` — Remaining unsafe are irreducible FFI and lifetime extension. [Phase 2 confirmed]
- `src/plan/global.rs` — Unsafe for SFT_MAP access and CommonPlan reference for work packet. [Phase 2 confirmed]
- `src/util/alloc/allocator.rs` — Unsafe impl Sync for AllocationOptionsHolder and raw memory write in fill_alignment_gap. [Phase 2 confirmed]
- `src/util/malloc/mod.rs` — Irreducible FFI wrappers. [Phase 2 confirmed]
- `src/mmtk.rs` — Irreducible UnsafeCell access and circular initialization. [Phase 2 confirmed]
- `src/policy/sft_map.rs` — Transmutes are irreducible due to fat pointer provenance. [Phase 2 confirmed]
- `src/util/erase_vm.rs` — Erased VM references are used to bypass generic type parameters in object-safe traits (SFT), and are necessary for performance and design. [Phase 2 confirmed]
- `src/policy/vmspace.rs` — Irreducible SFT initialization. [Phase 2 confirmed]
- `src/util/int_array_freelist.rs` — No unsafe code found. [Phase 2 confirmed]
- `src/util/heap/layout/mmapper/csm/two_level_storage.rs` — No unsafe code found after removing redundant unsafe impls. [Phase2 confirmed]
- `src/vm/slot.rs` — SimpleSlot is a safe abstraction. Investigation showed that using references adds lifetime burden and moves unsafe to construction. [Phase 2 confirmed]

## Abstraction Proposals (for Phase 2)
### StwProof for safe plan access
- Target files: `src/mmtk.rs`, `src/scheduler/gc_work.rs`, `src/scheduler/scheduler.rs`
- Expected Δ: -4
- Design sketch: Introduce a zero-sized `StwProof` token that can only be obtained when GC is in progress. Use it to access plan mutably.
- Status: done
