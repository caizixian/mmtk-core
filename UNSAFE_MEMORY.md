# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 351 | Current: 302 | Δ: -49
- Phase: 2

## Codebase Invariants (PROTECTED — do not prune)
Architectural insights that affect ALL future safety decisions:
- Work packets hold raw pointers to plans or spaces to bypass borrow checker and lifetimes.
- Static plan references are needed because plan types are generic and cannot be stored in global statics easily.

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🔴 HIGH: `src/util/metadata/pin_bit.rs:39` — investigate if we can use safe atomic load for pin bit — expected Δ: -1
2. 🟡 MED: `src/vm/object_model.rs:160` — investigate if we can use safe atomic load for metadata spec — expected Δ: -1

## Patterns Discovered
Reusable refactoring patterns (recipe format):
- Introducing `StwProof` token to encapsulate raw memory operations or mutable access to shared structures (like `Plan`) during Stop-The-World phases.
- Introducing `MetadataSlot` abstraction to encapsulate raw memory operations on metadata addresses behind a safe API.
- Safe wrappers in `Mutator` (like `get_allocator_mut_safe`) can encapsulate `unsafe` array access by checking initialization against `space_mapping`.
- Extending `MetadataSlot` with generic methods for `MetadataValue` allows centralizing unsafe operations on types larger than `u8`.
- Using safe wrappers in `Mutator` (like `allocator_impl_mut_for_semantic`) in plan-specific mutators to eliminate direct unsafe calls to `allocators.get_allocator_mut`.
- Replacing platform-specific FFI calls with safe standard library equivalents (e.g. `std::thread::available_parallelism` instead of `sched_getaffinity`).
- Replacing `ObjectReference::from_raw_address_unchecked` with `ObjectReference::from_raw_address(...).unwrap()` when the address is guaranteed to be non-zero (e.g. checked by assertion or invariant).

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/plan/concurrent/concurrent_marking_work.rs` — `ConcurrentTraceObjects` uses raw pointer to bypass borrow checker for work packet. [Phase 2 confirmed]
- `src/policy/sft_map.rs` — `SFTRefStorage` uses transmute for atomic fat pointers. [Phase 2 confirmed]
- `src/util/metadata/metadata_val_traits.rs` — Irreducible raw loads from addresses in trait default impls. [Phase 2 confirmed]
- `src/util/memory.rs` — Contains wrappers for FFI calls. [Phase 2 confirmed]
- `docs/dummyvm/src/api.rs` — Irreducible FFI boundaries in dummy VM. [Phase 2 confirmed]
- `src/util/malloc/malloc_ms_util.rs` — Irreducible FFI and raw pointer manipulation. [Phase 2 confirmed]
- `src/util/address.rs` — Primitives for address operations. [Phase 2 confirmed]
- `src/util/metadata/side_metadata/side_metadata_tests.rs` — Tests use unsafe to check address iteration. [Phase 2 confirmed]
- `src/util/rust_util/atomic_box.rs` — Custom lock-free lazily initialized box. [Phase 2 confirmed]
- `src/util/test_util/fixtures.rs` — Fixtures use unsafe for test setup. [Phase 2 confirmed]
- `src/vm/slot.rs` — `SimpleSlot` is a safe abstraction. [Phase 2 confirmed]
- `src/util/rust_util/mod.rs` — `InitializeOnce` is irreducible for performance. [Phase 2 confirmed]
- `src/util/heap/blockpageresource.rs` — UnsafeCell accesses in lock-free queue. [Phase 2 confirmed]
- `src/policy/marksweepspace/native_ms/block.rs` — Raw memory accesses for free list. [Phase 2 confirmed]
- `src/util/metadata/side_metadata/global.rs` — Remaining unsafe are irreducible function signatures and raw memory copy. [Phase 2 confirmed]
- `src/policy/marksweepspace/malloc_ms/global.rs` — Remaining unsafe are irreducible FFI and lifetime extension. [Phase 2 confirmed]
- `src/policy/copyspace.rs` — Remaining unsafe are irreducible FFI and lifetime extension. [Phase 2 confirmed]
- `src/plan/global.rs` — Unsafe for SFT_MAP access and CommonPlan reference for work packet. [Phase 2 confirmed]
- `src/util/alloc/allocator.rs` — Unsafe impl Sync for AllocationOptionsHolder and raw memory write in fill_alignment_gap. [Phase 2 confirmed]
- `src/util/malloc/mod.rs` — Irreducible FFI wrappers. [Phase 2 confirmed]
- `src/mmtk.rs` — Irreducible UnsafeCell access and circular initialization. [Phase 2 confirmed]

## Abstraction Proposals (for Phase 2)
### StwProof for safe plan access
- Target files: `src/mmtk.rs`, `src/scheduler/gc_work.rs`, `src/scheduler/scheduler.rs`
- Expected Δ: -4
- Design sketch: Introduce a zero-sized `StwProof` token that can only be obtained when GC is in progress. Use it to access plan mutably.
- Status: done
