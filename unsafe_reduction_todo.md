# Unsafe Reduction: Prioritized TODO Checklist

This is an ordered list of proposed unsafe reduction changes in mmtk-core, ranked from
highest priority to lowest priority based on these criteria (most important first):

1. **Meaningful unsafe reduction** — Does this genuinely reduce unsafe or make it easier to audit?
   Or is it just unsafe shuffling (e.g., removing a design marker)?
2. **Upstream perception** — Is this straightforwardly an improvement and easy to review?
   Or would it be questionable for a performance-sensitive systems project?
3. **Performance impact** — Is this likely zero-cost, or does it need benchmarking?

---

## Tier 1: Clearly Beneficial, Easy to Review, Zero/Minimal Performance Impact

- [ ] **1. Safe Initialization (MaybeUninit → Option)** (Δ = -30)
  Replaces `MaybeUninit` arrays with `Option` arrays in allocators and copy contexts. This is a
  genuine safety improvement: `MaybeUninit::assume_init()` is a common source of UB when
  misused, and `Option` provides runtime-checked access. The change is straightforward and
  idiomatic Rust. Performance impact is negligible — `Option<T>` for non-hot-path allocator
  initialization has no measurable overhead. The only caveat is layout: `Option<T>` may differ
  from `MaybeUninit<T>` in size if `T` doesn't have a niche, but since the existing code already
  uses fixed-size arrays and the report shows it compiles fine, this is acceptable.
  *Note: this is already implemented as a branch (`unsafe_maybeuninit_option`).*

- [ ] **2. Interior Mutability (UnsafeCell → Mutex)** (Δ = -22)
  Replaces `UnsafeCell` + manual `Mutex<()>` pattern with proper `Mutex<T>` in `Map32` and
  `Map64`. This genuinely eliminates unsound patterns: `UnsafeCell::get()` with manual locking
  is error-prone and bypasses Rust's borrow checking. The `Mutex<T>` pattern is idiomatic and
  the compiler can verify safety. These maps are only accessed during space creation and GC
  (not on the allocation fast path), so the Mutex overhead is irrelevant. Also removes
  `unsafe impl Send/Sync` which were required due to `UnsafeCell`. Easy to review as a
  clear improvement.

- [ ] **3. Safe Trait Abstraction for Atomics** (Δ = -20)
  Replaces `MetadataValue` trait methods that take raw `Address` and do unsafe pointer casts
  with methods that take safe references (`&Self::Atomic`). This is a genuine API improvement:
  the old trait required every implementor to do unsafe pointer-to-atomic casts, and the new
  design uses Rust's type system to ensure safety. The change is entirely at the trait level
  and doesn't affect runtime behavior. Zero performance impact.

- [ ] **4. Safe Initialization (OnceLock)** (Δ = -13)
  Replaces `UnsafeCell<MaybeUninit<T>>` + `Once` with `std::sync::OnceLock<T>` for
  `InitializeOnce`, `VMLayout`, and `TwoLevelStateStorage`. This eliminates unsafe pointer
  gymnastics for lazy initialization. `OnceLock` is standard library, well-tested, and
  idiomatic. Minor concern: `OnceLock::get()` does an atomic load on every access (vs. the
  original direct dereference after `Once` completion), but for read-heavy paths like
  `vm_layout()`, the single atomic load is essentially free on modern hardware. Easy PR.

- [ ] **5. Trait Safety Relaxation (Auto-Traits)** (Δ = -25)
  Removes explicit `unsafe impl Send/Sync` across many types because the underlying fields now
  use safe concurrent types. This is a *consequence* of other changes — once fields are safe,
  the compiler can auto-derive `Send/Sync`. Each removal is trivially correct and reviewable.
  Zero performance impact. Should be bundled with the changes that enable each removal. Some
  removals (like `OpaquePointer` changing from `*mut c_void` to `usize`) are standalone.

- [ ] **6. Safe Precondition Enforcement (Runtime Checks)** (Δ = -18)
  Replaces `unsafe` allocator access methods (`get_allocator`, `allocator_mut`, etc.) with safe
  versions that do runtime assertions. The unsafe was not about memory safety — it was a
  documentation mechanism ("you must pass a valid selector"). The runtime checks make the
  contract explicit and debuggable. Performance note: these assertions run on the allocation
  path, but `assert!` checks against `space_mapping` are very cheap (small array linear scan).
  Could gate behind `debug_assert!` if concerned. Clean PR.

- [ ] **7. Safe Concurrent Data Structures (Crossbeam ArrayQueue)** (Δ = -8)
  Replaces custom `BlockQueue` using `UnsafeCell<MaybeUninit>` and `push_relaxed` with
  `crossbeam::ArrayQueue`. The custom implementation was genuinely unsafe (manually managing
  init state of array elements). The crossbeam replacement is well-audited and widely used.
  Performance: `ArrayQueue` is a bounded lock-free queue — it should match or beat the custom
  implementation. Adding a dependency (crossbeam) is the only reviewer concern, but crossbeam
  is standard in the Rust ecosystem.

- [ ] **8. Safe Shared State via Arc/RwLock** (Δ = -4)
  Replaces `NonNull` parent pointer in `IntArrayFreeList` with `Arc<RwLock<Vec<i32>>>`. The
  old design had a raw pointer to parent's `Vec` — genuinely unsound if parent is moved or
  dropped. The `Arc<RwLock>` is correct. **High performance concern**: `RwLock` is acquired on
  *every* `get_entry`/`set_entry` call, which are in the freelist allocation hot path. The old
  code was a zero-cost raw pointer dereference. Even though freelist operations often happen
  under a page resource lock (reducing contention), the locking overhead itself is significant.
  **Definitely needs benchmarking** for all plans using `FreeListPageResource` (MarkSweep,
  Immix, etc). Consider alternative approaches: e.g., unsafe `UnsafeCell` with documented
  invariants, or restructuring to avoid shared mutable state entirely.

- [ ] **9. Safe Standard Library Alternatives** (Δ = -5)
  Replaces libc FFI calls (`getpid`/`gettid`, `memset`, `sched_getaffinity`) with std library
  equivalents (`std::process::id()`, `slice::fill`, `available_parallelism()`). Each replacement
  is clearly better: safer, more portable, and often more readable. Zero performance impact.
  Dead-simple to review.

- [ ] **10. Safe Initialization via Vector** (Δ = -6)
  Replaces manual `std::alloc::alloc_zeroed` with `Vec` or `bytemuck::zeroed_vec`. Eliminates
  raw pointer manipulation for buffer initialization. Mostly in benchmarks and test utilities.
  Clearly correct and easy to review. Zero performance impact (benchmarks allocate once).

- [ ] **11. Safe Initialization (NonZeroUsize)** (Δ = -1)
  Replaces `NonZeroUsize::new_unchecked` with `NonZeroUsize::new().expect()`. Tiny but
  genuine: the unchecked version is UB if passed zero; the checked version panics. The
  `debug_assert` guard was already there, so this just makes it unconditional. Zero perf impact
  (called during region creation, not hot path).

- [ ] **12. Safe Enum Conversion** (Δ = -2)
  Replaces `unsafe impl ZeroableInOption/PodInOption` for `Pause` enum with explicit safe
  conversion methods. The bytemuck impls were asserting memory layout invariants that the
  compiler couldn't verify. The safe methods are trivial match arms. Zero performance impact.

- [ ] **13. Safe Trait Implementation (Derive)** (Δ = -1)
  Replaces `unsafe impl Zeroable for SpaceDescriptor` with `#[derive(Zeroable)]`. The derive
  macro verifies all fields are `Zeroable`. Trivially correct. Zero performance impact.

- [ ] **14. Safe Type Erasure (std::any::Any)** (Δ = -1)
  Replaces raw pointer provenance tricks with `&mut dyn Any` + `downcast_mut`. The old pattern
  was genuinely unsafe (provenance erasure). The new pattern uses Rust's type system. Minimal
  performance impact (one virtual dispatch). Clean improvement.

- [ ] **15. Safe Test Fixtures (Leaked References)** (Δ = -4)
  Replaces raw pointers with `Box::leak` in test fixtures. Tests can afford to leak. Eliminates
  unsafe dereferences and manual Drop. Test-only, zero production impact.

- [ ] **16. Mock Slots: Raw Pointers to References** (Δ = -11)
  Replaces raw pointers with safe references (with lifetimes) in mock test slot
  implementations. Test-only code, but the change is an excellent example of safe Rust. Zero
  production impact. Easy to review.

---

## Tier 2: Genuine Improvements, Moderate Complexity or Minor Performance Considerations

- [ ] **17. SFT Map Refactoring (AtomicPtr & Wrapper)** (Δ = -38)
  Replaces `AtomicDoubleWord` + `transmute` for SFT fat pointers with `AtomicPtr<SFTWrapper>`
  using thin pointers to leaked wrapper structs. This is a real improvement: the old code
  transmuted fat pointers to/from `DoubleWord` which was fragile and platform-specific. The new
  design uses standard `AtomicPtr` and `Box::leak`. The `get_sft_wrapper` function uses a
  `Mutex<HashMap>` for deduplication (only during space init), and the `SFTRefStorage::load`
  still has one `unsafe` (dereferencing the `AtomicPtr`), but the invariant is much easier to
  audit. Performance: the load path has one extra pointer indirection (ptr → wrapper → dyn SFT)
  vs the old transmute-to-fat-ptr. SFT lookup is on the object tracing hot path — **needs
  benchmarking**. The HashMap+Mutex in `get_sft_wrapper` is init-only.

- [ ] **18. Proof Token and StwProtected Abstraction** (Δ = -7)
  Introduces a zero-sized `StwProof` token and `StwProtected<T>` wrapper to guard mutable
  access to the Plan during STW. Replaces raw `*const Plan as *mut Plan` casts. This is a
  genuinely better pattern — it encodes the "world is stopped" invariant in the type system.
  The remaining unsafes in `StwProtected` are well-localized and easy to audit. Performance:
  zero-cost (proof token is ZST). Complexity concern: adds new types to the API surface,
  requires understanding the proof-carrying pattern. Moderate review effort.

- [ ] **19. API Refactoring (Safe References)** (Δ = -4)
  Passes references instead of raw pointers to methods (e.g., `Block::attempt_release` now
  takes `&mut BlockList` instead of dereferencing a raw pointer, `ProcessEdgesBase` removes
  the raw `*mut GCWorker` field). Genuine safety improvement. The `ScanMutatorRoots` change
  (using `Option<&'static mut Mutator>` + `take()`) is elegant. Moderate review effort due to
  API surface changes. Performance: the `ProcessEdgesBase` change removes a raw pointer
  dereference that was specifically kept for "fast pointer dereferencing" during copying GC.
  The comment in the original code explicitly mentioned this optimization — **needs
  benchmarking** for copying GC workloads.

- [ ] **20. Safe FFI Signatures** (Δ = -9)
  Replaces raw pointer params in `extern "C"` functions with `Option<&mut T>` / `Option<Box<T>>`.
  These are ABI-compatible with nullable C pointers, so the FFI contract is preserved. The
  panicking `.expect("..is null")` is arguably better than silent UB on null. Note: this only
  affects the dummyvm example, not production code. But it sets a good pattern for bindings.
  Easy to review. Zero performance impact.

- [ ] **21. Encapsulated Allocator Access** (Δ = -9)
  Adds `allocator_impl_mut_for_semantic` helper on `Mutator` to replace repeated
  `unsafe { get_allocator_mut(...) }.downcast_mut::<T>().unwrap()` patterns across all
  plan mutator files. Eliminates 9 unsafe blocks across 8 files. The unsafe was from accessing
  uninitialized MaybeUninit allocators — which is genuinely fixed by the MaybeUninit→Option
  change (item 1). This change is downstream of item 1. Easy to review, zero perf impact.

- [ ] **22. Removal of Self-Reference Casts** (Δ = -6)
  Eliminates `unsafe { &*(self as *const Self) }` patterns used to create 'static references
  to pass to work packets. The fix varies: ImmixSpace uses PhantomData, MarkSweepSpace uses
  `Arc<Inner>`. This is a genuine improvement — self-reference casts are fragile and can cause
  UB if the object moves. The Arc approach adds reference counting overhead but only during
  GC work packet creation (not hot path). The ImmixSpace PhantomData approach is zero-cost.
  Moderate review effort due to architectural changes in MarkSweepSpace.

- [ ] **23. Safe Lifetime Enforcement** (Δ = -1)
  Changes `CopySpaceCopyContext::rebind` to require `&'static CopySpace<VM>` instead of
  erasing the lifetime via raw pointer cast. Genuine fix — the old code was transmuting
  away a lifetime bound. Requires callers to prove the CopySpace is static (which it is).
  Easy to review. Zero perf impact.

- [ ] **24. Removal of Static Plan Hack** (Δ = -3)
  Removes unsafe cast of `&Plan` to `&'static Plan` and `Arc::as_ptr` mutation for
  GCTrigger initialization. The fix restructures MMTK initialization to avoid the circular
  dependency. Genuine safety improvement. Moderate review effort (touches MMTK init). Zero
  performance impact.

- [ ] **25. Safe Iterator Abstraction (Block Cells)** (Δ = -4)
  Replaces manual pointer arithmetic in sweeping loops with a safe `CellIter` and safe
  `ObjectReference::from_raw_address().unwrap()`. Genuine improvement — eliminates
  `from_raw_address_unchecked`. Performance: `unwrap()` adds a branch per object in the sweep
  loop. Sweep is GC-phase-only, so impact is limited, but could be measurable on large heaps.
  **May need benchmarking** for mark-sweep workloads.

- [ ] **26. Safe Method Signatures (Internal Helpers)** (Δ = -9)
  Removes `unsafe` markers from methods like `allocate_contiguous_chunks`,
  `free_contiguous_chunks`, `reset`, `get_current_chunk` on page resources and copyspace.
  The old `unsafe` markers were not about memory safety but about threading invariants (now
  enforced by `Mutex`). Downstream of the Map32/Map64 Mutex change (item 2). Easy to review.
  Zero performance impact.

- [ ] **27. Safe API: VMMap Methods** (Δ = -2)
  Makes `allocate_contiguous_chunks` and `free_contiguous_chunks` safe on the `VMMap` trait.
  Downstream of interior mutability fix (item 2). Easy to review. Zero performance impact.

- [ ] **28. Encapsulation of OS Memory Management** (Δ = -6)
  Consolidates multiple adjacent `unsafe` libc calls (mmap, prctl, madvise) into single
  `unsafe` blocks. Doesn't eliminate unsafety — OS calls are inherently unsafe. But improves
  readability and makes the safety arguments clearer. Also wraps `mprotect` in a safe
  internal helper. Easy to review. Zero performance impact.

- [ ] **29. Safe Malloc/Calloc Wrappers** (Δ = -5)
  Wraps C allocator calls (`calloc`, `free`) in safe Rust helpers. The unsafety is inherent
  in C FFI, but the wrappers centralize the unsafe boundary. Moderate improvement. Easy PR.

- [ ] **30. Safe OS Abstractions (core_affinity)** (Δ = -1)
  Replaces raw libc FFI for `sched_setaffinity` with the `core_affinity` crate. Adds a
  dependency but eliminates unsafe. Only used during GC worker thread setup (not hot path).
  The `bind_current_thread_to_cpuset` still uses raw libc. Moderate improvement.

---

## Tier 3: Debatable or Low-Value Changes

- [ ] **31. Safe Metadata Abstraction (MetadataSlot)** (Δ = -188)
  The largest numerical reduction, but also the most debatable. Creates a `MetadataSlot(Address)`
  wrapper that encapsulates `unsafe { addr.as_ref::<AtomicU8>() }` etc. The unsafe isn't
  eliminated — it's moved into MetadataSlot's `get_ref`/`get_mut_ref` methods with
  `unsafe { self.0.as_ref::<T>() }`. The benefit is localization: instead of 188 scattered
  unsafe blocks, there are ~5 unsafe blocks inside MetadataSlot. However, the invariant
  ("this Address points to valid mapped metadata memory") is **not checked** — it's trusted.
  The wrapper provides no additional safety guarantees beyond the Address type itself. This is
  primarily unsafe *centralization* not unsafe *elimination*. Still valuable for auditability
  if the wrapper's invariants are well-documented. Some changes in this category also
  silently convert non-atomic loads to atomic (Relaxed), which changes semantics. **Needs
  careful review** for correctness and potential performance impact of added atomics.

- [ ] **32. Safe Metadata Abstraction (SideMetadataSpecBlockExt)** (Δ = -16)
  Similar to MetadataSlot — creates a trait extension that wraps unsafe metadata access in
  safe methods. The unsafe is centralized in the trait implementation, not eliminated. The
  invariant (valid metadata mapping) is still trusted. Moderate improvement for auditability.

- [ ] **33. Atomic Side Metadata Access** (Δ = -10)
  Replaces non-atomic loads/stores with `Ordering::Relaxed` atomics on VO bit metadata.
  This changes the memory model semantics, not just the syntax. On x86, `Relaxed` atomic
  loads compile to plain `mov` (same as non-atomic), but on weaker architectures (ARM),
  this may add fences. The change addresses data races at the language level (Miri would
  flag the old code), which is a genuine improvement. But it's a semantics change, not
  just syntax — **reviewers may question** whether relaxed ordering is sufficient. Moderate
  complexity. **May need architecture-specific benchmarking** on ARM.

- [ ] **34. Safe Metadata API (Malloc MS)** (Δ = -15)
  Replaces `is_marked_unsafe`, `unset_vo_bit_unsafe` etc. with safe versions using atomic
  ops. Similar to item 33 — changes semantics from non-atomic to atomic. The `_unsafe` suffix
  in the original was a naming convention for non-atomic access, not for Rust unsafety per se.
  Moderate improvement but changes behavior.

- [ ] **35. Atomic Operations (load_atomic/store_atomic)** (Δ = -8)
  Same pattern as 33 and 34: replaces unsafe non-atomic access with safe atomic access for
  line marks, chunk map, pin bit, log bit, and object model. Each is a semantic change. Most
  use `Ordering::Relaxed` which should be zero-cost on x86 but **needs care on ARM**.

- [ ] **36. Safe Address Constructors** (Δ = -138)
  Makes `Address::from_usize()`, `Address::zero()`, and `Address::max()` safe functions. This
  is the most contentious change. The original `unsafe` on `Address::from_usize` was a
  **deliberate design marker** — it signaled "this address is manufactured from thin air and
  the caller must justify its validity." Removing `unsafe` removes this documentation signal.
  The vast majority of the 138 reduction is in tests and constants where the addresses are
  obviously valid (hardcoded hex constants, `Address::zero()` sentinel values). These are
  clearly safe. But the design-marker function of the unsafe is lost for production code too.
  An upstream reviewer may view this as **removing a useful safety annotation** rather than
  a genuine unsafe reduction. The counter-argument: `Address` doesn't implement `Deref`,
  so creating an `Address` value can never cause UB — it's just a `usize` wrapper.
  Therefore, the `unsafe` marker was technically incorrect. Zero performance impact.

- [ ] **37. Safe Object Reference Creation** (Δ = -3)
  Replaces `ObjectReference::from_raw_address_unchecked` with
  `ObjectReference::from_raw_address().unwrap()`. The `unwrap()` adds a zero-check branch.
  Genuine improvement in the general case, but the original `_unchecked` was used in hot paths
  (object forwarding, sweep loops) where the caller had already established non-nullity.
  **May need benchmarking** for copying GC workloads if used in the forwarding pointer path.

- [ ] **38. Safe Slot Abstraction (SimpleSlot)** (Δ = -5)
  Changes `SimpleSlot` from storing `*mut Atomic<Address>` to storing `Address`, with a
  single `as_atomic()` helper that does the unsafe cast. Also removes `impl Slot for Address`.
  The unsafe is centralized, not eliminated. But the removal of `unsafe impl Send` and the
  cleaner API are genuine improvements. The `impl Slot for Address` removal is a breaking
  API change for bindings — **needs coordination with downstream**. Zero perf impact.

- [ ] **39. Slice Abstraction (Raw Pointer to Slice)** (Δ = -1)
  Replaces raw pointer arithmetic in `RawMemoryFreeList` with a `&'static mut [i32]` slice.
  The `unsafe { std::slice::from_raw_parts_mut(...) }` is still there, but subsequent accesses
  get bounds checking. Small but genuine improvement. The `'static mut` slice from mmap'd
  memory is sound if the mapping lifetime is correct. Low impact change.

- [ ] **40. Removal of Complex Bulk Metadata Operations** (Δ = -4)
  Removes `load128` and bulk XOR operations, falling back to per-object sweeping. This removes
  unsafe but may **regress performance** for malloc mark-sweep sweeping. The bulk operations
  were specifically an optimization. The report acknowledges this trade-off. **Needs
  benchmarking** for malloc mark-sweep workloads.

- [ ] **41. Consolidation of Unsafe Blocks** (Δ = -2)
  Merges adjacent unsafe blocks in `malloc_ms_util.rs`. Doesn't reduce actual unsafety at all
  — just reduces the count of `unsafe` keywords. Cosmetic improvement for readability.

- [ ] **42. Redundant Unsafe Cleanup** (Δ = -2)
  Removes `unsafe` blocks around already-safe function calls (like `Address::zero()`). These
  were leftover from when `Address::zero()` was unsafe. Trivially correct *if item 36 is
  accepted*. Dependent on item 36.

- [ ] **43. Removal of Unsafe Optimization** (Δ = -1)
  Removes the `set_raw_byte_atomic` optimization in `mark_byte_as_unlogged`, falling back to
  the safe `mark_as_unlogged` method. The optimization set an entire metadata byte to 0xFF
  (valid because all objects in mature space have log bit = 1). The safe fallback does a
  CAS loop to set individual bits. **May regress generational GC performance** if this is
  called frequently during nursery collection. **Needs benchmarking**.

- [ ] **44. Safe Slice/Array Access** (Δ = -1)
  Replaces `unsafe { *(typename.as_ptr()) }` with `typename.as_bytes().first().copied()` in a
  `black_box` workaround for bpftrace. The old code was a deliberate force-load; the new code
  achieves the same effect safely. Trivially correct. Zero performance impact (it's inside
  `black_box`).

---

## Summary Table

| Rank | Category | Δ | Genuine? | Easy PR? | Perf Risk? |
|------|----------|---|----------|----------|------------|
| 1 | Safe Initialization (MaybeUninit → Option) | -30 | ✅ | ✅ | None |
| 2 | Interior Mutability (UnsafeCell → Mutex) | -22 | ✅ | ✅ | None |
| 3 | Safe Trait Abstraction for Atomics | -20 | ✅ | ✅ | None |
| 4 | Safe Initialization (OnceLock) | -13 | ✅ | ✅ | Minimal |
| 5 | Trait Safety Relaxation (Auto-Traits) | -25 | ✅ | ✅ | None |
| 6 | Safe Precondition Enforcement (Runtime Checks) | -18 | ✅ | ✅ | Minimal |
| 7 | Safe Concurrent Data Structures (ArrayQueue) | -8 | ✅ | ✅ | None |
| 8 | Safe Shared State via Arc/RwLock | -4 | ✅ | ✅ | **Benchmark** |
| 9 | Safe Standard Library Alternatives | -5 | ✅ | ✅ | None |
| 10 | Safe Initialization via Vector | -6 | ✅ | ✅ | None |
| 11 | Safe Initialization (NonZeroUsize) | -1 | ✅ | ✅ | None |
| 12 | Safe Enum Conversion | -2 | ✅ | ✅ | None |
| 13 | Safe Trait Implementation (Derive) | -1 | ✅ | ✅ | None |
| 14 | Safe Type Erasure (std::any::Any) | -1 | ✅ | ✅ | None |
| 15 | Safe Test Fixtures (Leaked References) | -4 | ✅ | ✅ | None |
| 16 | Mock Slots: Raw Pointers to References | -11 | ✅ | ✅ | None |
| 17 | SFT Map Refactoring (AtomicPtr & Wrapper) | -38 | ✅ | Moderate | **Benchmark** |
| 18 | Proof Token and StwProtected Abstraction | -7 | ✅ | Moderate | None |
| 19 | API Refactoring (Safe References) | -4 | ✅ | Moderate | **Benchmark** |
| 20 | Safe FFI Signatures | -9 | ✅ | ✅ | None |
| 21 | Encapsulated Allocator Access | -9 | ✅ | ✅ | None |
| 22 | Removal of Self-Reference Casts | -6 | ✅ | Moderate | Low |
| 23 | Safe Lifetime Enforcement | -1 | ✅ | ✅ | None |
| 24 | Removal of Static Plan Hack | -3 | ✅ | Moderate | None |
| 25 | Safe Iterator Abstraction (Block Cells) | -4 | ✅ | ✅ | Low |
| 26 | Safe Method Signatures (Internal Helpers) | -9 | ✅ | ✅ | None |
| 27 | Safe API: VMMap Methods | -2 | ✅ | ✅ | None |
| 28 | Encapsulation of OS Memory Management | -6 | Partial | ✅ | None |
| 29 | Safe Malloc/Calloc Wrappers | -5 | Partial | ✅ | None |
| 30 | Safe OS Abstractions (core_affinity) | -1 | ✅ | ✅ | None |
| 31 | Safe Metadata Abstraction (MetadataSlot) | -188 | Shuffled | Complex | **Benchmark** |
| 32 | Safe Metadata Abstraction (BlockExt) | -16 | Shuffled | Moderate | Low |
| 33 | Atomic Side Metadata Access | -10 | Semantic Δ | Moderate | **Benchmark (ARM)** |
| 34 | Safe Metadata API (Malloc MS) | -15 | Semantic Δ | Moderate | **Benchmark (ARM)** |
| 35 | Atomic Operations (load_atomic/store_atomic) | -8 | Semantic Δ | Moderate | **Benchmark (ARM)** |
| 36 | Safe Address Constructors | -138 | Marker removal | Controversial | None |
| 37 | Safe Object Reference Creation | -3 | ✅ | ✅ | Low |
| 38 | Safe Slot Abstraction (SimpleSlot) | -5 | Shuffled | Breaking API | None |
| 39 | Slice Abstraction (Raw Pointer to Slice) | -1 | ✅ | ✅ | None |
| 40 | Removal of Complex Bulk Metadata Operations | -4 | ✅ | Controversial | **Benchmark** |
| 41 | Consolidation of Unsafe Blocks | -2 | Cosmetic | ✅ | None |
| 42 | Redundant Unsafe Cleanup | -2 | Depends on 36 | ✅ | None |
| 43 | Removal of Unsafe Optimization | -1 | ✅ | Questionable | **Benchmark** |
| 44 | Safe Slice/Array Access | -1 | ✅ | ✅ | None |
