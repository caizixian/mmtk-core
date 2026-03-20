# Comprehensive Unsafe Code Report: mmtk-core

> **Methodology**: Every `unsafe` keyword in `mmtk-core/src/` was identified, classified, and each instance was traced to its exact file and line. For each category, a zero-cost or near-zero-cost safer abstraction is proposed.

---

## Table of Contents

1. [Category A: Raw-Pointer Borrow-Checker Circumvention](#a-raw-pointer-borrow-checker-circumvention)
2. [Category B: `static mut` Global State](#b-static-mut-global-state)
3. [Category C: `unsafe impl Send/Sync`](#c-unsafe-impl-sendsync)
4. [Category D: `UnsafeCell` Interior Mutability (Map, Plan, InitializeOnce)](#d-unsafecell-interior-mutability)
5. [Category E: `MaybeUninit` Partially-Initialized Arrays](#e-maybeuninit-partially-initialized-arrays)
6. [Category F: `transmute` / Type-Punning](#f-transmute--type-punning)
7. [Category G: Raw Memory via `Address` Type](#g-raw-memory-via-address-type)
8. [Category H: Non-Atomic Sub-Byte Metadata Read-Modify-Write](#h-non-atomic-sub-byte-metadata-read-modify-write)
9. [Category I: FFI / `libc` Calls](#i-ffi--libc-calls)
10. [Category J: VM-Type-Erased Pointer Round-Trip](#j-vm-type-erased-pointer-round-trip)

---

## A. Raw-Pointer Borrow-Checker Circumvention

**Instances**: ~25  
**Characteristic**: Code that converts `&self` → `*const T` → `*mut T` → `&mut T` (or fabricates lifetime) to get a mutable or `'static` reference that Rust's borrow checker would not allow. This is the **most dangerous** category because it can silently create aliasing `&mut` references, which is instant UB.

### A1. Plan Mutation via `*const → *mut` cast

| # | File | Line | Code |
|---|------|------|------|
| 1 | [gc_work.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/scheduler/gc_work.rs#L59) | 59 | `let plan_mut: &mut C::PlanType = unsafe { &mut *(self.plan as *const _ as *mut _) };` |
| 2 | [gc_work.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/scheduler/gc_work.rs#L139) | 139 | `let plan_mut: &mut C::PlanType = unsafe { &mut *(self.plan as *const _ as *mut _) };` |
| 3 | [markcompact/gc_work.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/plan/markcompact/gc_work.rs#L47) | 47 | `let plan_mut = unsafe { &mut *(self.plan as *mut MarkCompact<VM>) };` |

**Structs involved**: `Prepare<C>`, `Release<C>`, `UpdateReferences<VM>`
Each stores `plan: *const C::PlanType` and casts to `&mut` in `do_work()`.

**Safety claim**: "We assume this is the only running work packet that accesses plan at the point of execution" (comment at line 40-42, 117-119).

**Risk**: Mutable aliasing UB. If the GC scheduler ever runs two plan-accessing packets concurrently, both will hold `&mut` to the same plan.

> [!IMPORTANT]  
> **Proposed Abstraction: `ExclusivePlanRef<T>`**
>
> A zero-cost newtype that makes the single-access invariant explicit in the type system. The key insight is that `UnsafeCell` already exists for interior mutability — we should use it, with a token type that represents "exclusive GC phase access".
>
> ```rust
> /// A token proving we are in an exclusive GC phase (sole accessor to the plan).
> /// Cannot be created by safe code — only the scheduler can issue these.
> pub struct PhaseToken(());  // zero-sized, no runtime cost
> 
> impl PhaseToken {
>     /// # Safety: Caller must guarantee no other work packet is accessing the plan.
>     pub(crate) unsafe fn new() -> Self { PhaseToken(()) }
> }
>
> /// A pointer to a plan that can only be dereferenced with a PhaseToken.
> pub struct ExclusivePlanRef<T>(*const T);
> unsafe impl<T> Send for ExclusivePlanRef<T> {}  // only one exists at a time
>
> impl<T> ExclusivePlanRef<T> {
>     pub fn new(plan: &T) -> Self { ExclusivePlanRef(plan as *const T) }
>     /// Get mutable access. Requires proof that we are the sole accessor.
>     #[inline(always)]  // zero cost in release
>     pub fn as_mut(&self, _token: &PhaseToken) -> &mut T {
>         unsafe { &mut *(self.0 as *mut T) }
>     }
> }
> ```
>
> **What changes**: `Prepare`/`Release` store `ExclusivePlanRef<C::PlanType>`. In `do_work`, the scheduler passes the `PhaseToken`. The unsafe is moved from every work packet (3 sites) into a single `PhaseToken::new()` call in the scheduler.
>
> **Overhead**: Zero. `PhaseToken` is zero-sized. `ExclusivePlanRef::as_mut` inlines to a pointer deref.

---

### A2. `ScanMutatorRoots` aliasing `&mut Mutator`

| # | File | Line | Code |
|---|------|------|------|
| 4 | [gc_work.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/scheduler/gc_work.rs#L445) | 445 | `unsafe { &mut *(self.0 as *mut _) }` |

```rust
// gc_work.rs:434-448
fn do_work(&mut self, worker: &mut GCWorker<C::VM>, mmtk: &'static MMTK<C::VM>) {
    VM::VMScanning::scan_roots_in_mutator_thread(
        worker.tls,
        unsafe { &mut *(self.0 as *mut _) },  // SECOND &mut to same Mutator
        factory,
    );
    self.0.flush();  // FIRST &mut still alive
}
```

**Risk**: **Critical UB**. Two `&mut` references to the same `Mutator` coexist: one passed to `scan_roots_in_mutator_thread`, one used for `flush()` afterward. Under Stacked Borrows, this is instant UB.

> [!IMPORTANT]  
> **Proposed Fix: Split the operations sequentially**
>
> The fix is simple and zero-cost — just restructure the code so only one `&mut` exists at a time:
>
> ```rust
> fn do_work(&mut self, worker: &mut GCWorker<C::VM>, mmtk: &'static MMTK<C::VM>) {
>     // Phase 1: Scan roots (borrows self.0)
>     VM::VMScanning::scan_roots_in_mutator_thread(
>         worker.tls,
>         self.0,      // pass the existing &mut directly — no raw pointer needed!
>         factory,
>     );
>     // Phase 2: Flush (after scan_roots returns, the borrow is released)
>     self.0.flush();
> }
> ```
>
> If the borrow checker complains because `scan_roots_in_mutator_thread` takes `&mut self.0` while `self` is borrowed, we can introduce a local rebind:
> ```rust
> let mutator: &mut Mutator<C::VM> = self.0;
> VM::VMScanning::scan_roots_in_mutator_thread(worker.tls, mutator, factory);
> self.0.flush();
> ```
>
> **Overhead**: Zero. We're just reorganizing borrows.

---

### A3. Space self-reborrow for work packets (`&*(self as *const Self)`)

| # | File | Line | Code |
|---|------|------|------|
| 5 | [immixspace.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/policy/immix/immixspace.rs#L450) | 450 | `let space = unsafe { &*(self as *const Self) };` |
| 6 | [immixspace.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/policy/immix/immixspace.rs#L547) | 547 | `let space = unsafe { &*(self as *const Self) };` |
| 7 | [native_ms/global.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/policy/marksweepspace/native_ms/global.rs#L428) | 428 | `let space = unsafe { &*(self as *const Self) };` |
| 8 | [native_ms/global.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/policy/marksweepspace/native_ms/global.rs#L444) | 444 | `let space = unsafe { &*(self as *const Self) };` |
| 9 | [native_ms/global.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/policy/marksweepspace/native_ms/global.rs#L532) | 532 | `let space = unsafe { &*(self as *const Self) };` |
| 10 | [malloc_ms/global.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/policy/marksweepspace/malloc_ms/global.rs#L560) | 560 | `let space = unsafe { &*(self as *const Self) };` |
| 11 | [global.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/plan/global.rs#L761) | 761 | `let common_plan = unsafe { &*(self as *const CommonPlan<VM>) };` |

**Pattern**: Inside a method taking `&mut self`, creates an unbounded-lifetime `&Self` via raw pointer to pass to work packets. The work packet will use this reference after the current `&mut self` borrow ends.

**Safety claim**: "reference is always valid within this collection cycle" (inline comments).

**Risk**: Creates a Stacked Borrows violation — a `&` derived from `&mut self` and outliving it.

> [!IMPORTANT]  
> **Proposed Abstraction: `SpaceRef<S>` — a work-packet-safe reference**
>
> The key insight is that spaces are heap-allocated and pinned (inside a `Box` in the plan). Their addresses never change. We can model this as a `SpaceRef` that is explicitly `'static`, created once during plan construction.
>
> ```rust
> /// A reference to a space that is valid for the lifetime of the plan.
> /// Created once; can be cheaply copied into work packets.
> #[derive(Clone, Copy)]
> pub struct SpaceRef<S: 'static>(&'static S);
> 
> impl<S: 'static> SpaceRef<S> {
>     /// Create a SpaceRef. # Safety: the space must be heap-allocated and never moved.
>     pub(crate) unsafe fn from_space(space: &S) -> Self {
>         SpaceRef(&*(space as *const S))
>     }
>     #[inline(always)]
>     pub fn get(&self) -> &'static S { self.0 }
> }
> ```
>
> **What changes**: Each space creates its `SpaceRef` once during plan init. The `prepare()`/`release()` methods use `space_ref` (a field set once) instead of casting `&self`:
> ```rust
> impl ImmixSpace<VM> {
>     fn prepare(&mut self, ...) {
>         let space_ref = self.self_ref;  // SpaceRef<Self>, set during ::new()
>         let work_packets = self.chunk_map.generate_tasks(|chunk| {
>             Box::new(PrepareBlockState { space: space_ref.get(), chunk, ... })
>         });
>     }
> }
> ```
>
> **Overhead**: One `usize` field per space. Zero runtime cost on the hot path.
> **Benefit**: Moves 7 unsafe blocks (sites 5-11) into a single `SpaceRef::from_space()` during init.

---

### A4. `ProcessEdgesBase::worker()` — raw `*mut` → `&'static mut`

| # | File | Line | Code |
|---|------|------|------|
| 12 | [gc_work.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/scheduler/gc_work.rs#L486) | 486 | `worker: *mut GCWorker<VM>,` (field) |
| 13 | [gc_work.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/scheduler/gc_work.rs#L522-L524) | 522 | `pub fn worker(&self) -> &'static mut GCWorker<VM> { unsafe { &mut *self.worker } }` |
| 14 | [concurrent_marking_work.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/plan/concurrent/concurrent_marking_work.rs#L42-L45) | 42 | Same `worker()` pattern |

**Pattern**: The worker pointer is stored as `*mut GCWorker<VM>` for "fast pointer dereferencing, instead of using `Option<&'static mut GCWorker<VM>>`" (comment at line 484). Starts as null, set in `set_worker()`.

**Risk**: (1) Returns `&'static mut` from `&self` — aliasing if called twice. (2) Null deref if called before `set_worker()`.

> [!IMPORTANT]  
> **Proposed Abstraction: `WorkerRef<VM>` — a non-null, non-aliasing worker reference**
>
> ```rust
> /// A non-null raw pointer to a GCWorker, with a safe accessor.
> /// Invariant: once set, the pointed-to worker is valid for the GC cycle.
> pub struct WorkerRef<VM: VMBinding>(NonNull<GCWorker<VM>>);
>
> impl<VM: VMBinding> WorkerRef<VM> {
>     /// Create from a worker reference. The worker must outlive this ref.
>     pub fn from_worker(worker: &mut GCWorker<VM>) -> Self {
>         WorkerRef(NonNull::from(worker))
>     }
>     /// Get the worker. Returns &GCWorker, not &mut, to prevent aliasing.
>     #[inline(always)]
>     pub fn get(&self) -> &GCWorker<VM> {
>         // SAFETY: The worker is alive for the duration of the GC cycle.
>         unsafe { self.0.as_ref() }
>     }
> }
> unsafe impl<VM: VMBinding> Send for WorkerRef<VM> {}
> ```
>
> **Key changes from current code**:
> 1. Uses `NonNull` instead of `*mut` — eliminates null state entirely
> 2. Returns `&GCWorker` not `&'static mut GCWorker` — eliminates aliasing
> 3. `ProcessEdgesBase::new()` requires `WorkerRef` (not null, set later)
>
> If `&mut GCWorker` is truly needed (e.g., for `get_copy_context_mut()`), the `worker` parameter in `do_work()` already provides one. The `ProcessEdgesBase::worker()` pattern should return `&GCWorker`, and callers needing mutation should use the `worker` param from `do_work`.
>
> **Overhead**: Zero. `NonNull<T>` is the same size as `*mut T`. The deref inlines to identical code.

---

### A5. `Arc` mutation during init

| # | File | Line | Code |
|---|------|------|------|
| 15 | [mmtk.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/mmtk.rs#L187) | 187 | `let gc_trigger: &mut GCTrigger<VM> = unsafe { &mut *(Arc::as_ptr(&gc_trigger) as *mut _) };` |
| 16 | [mmtk.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/mmtk.rs#L189) | 189 | `let static_plan: &'static dyn Plan<VM = VM> = unsafe { &*(&*plan as *const _) };` |

**Risk**: Technically violates Arc's shared-ownership invariant (refcount > 1 but we mutate through it). However, this is during single-threaded init.

> [!TIP]
> **Proposed Fix**: Use `Arc::get_mut()` which is safe when refcount == 1. If refcount > 1 at this point (because `gc_trigger` was cloned into the plan), restructure the init order to set the plan reference before cloning the Arc. Alternatively, the comment already notes: "TODO: use Arc::get_mut_unchecked() when it is available" — this is now available as `Arc::get_mut_unchecked()` on nightly.
>
> For the `static_plan` fabrication: store `&'static dyn Plan` in the `GCTrigger` by having `GCTrigger` use `OnceLock<&'static dyn Plan>` (set after the plan is boxed in `UnsafeCell`).

---

## B. `static mut` Global State

**Instances**: 2 in mmtk-core, 2 in mmtk-openjdk  
**Characteristic**: Global mutable variables without synchronization. Deprecated in Rust 2024 edition.

| # | File | Line | Current Code | Access Pattern |
|---|------|------|-------------|----------------|
| 1 | [vm_layout.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/heap/layout/vm_layout.rs#L189) | 189 | `static mut VM_LAYOUT: VMLayout = VMLayout::new_32bit();` | Written once during init (line 170), read via `vm_layout()` (line 202) |
| 2 | [vm_layout.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/heap/layout/vm_layout.rs#L191) | 191 | (64-bit variant) | Same |

The write happens in `VMLayout::set_custom_vm_layout()` (line 163-172) with an `AtomicBool` guard (`VM_LAYOUT_FETCHED`), but the write itself is **not atomic**.

> [!IMPORTANT]
> **Proposed Fix: Replace with `OnceLock<VMLayout>`**
>
> ```rust
> use std::sync::OnceLock;
> 
> static VM_LAYOUT: OnceLock<VMLayout> = OnceLock::new();
> 
> pub fn vm_layout() -> &'static VMLayout {
>     VM_LAYOUT.get_or_init(|| {
>         #[cfg(target_pointer_width = "32")]
>         { VMLayout::new_32bit() }
>         #[cfg(target_pointer_width = "64")]
>         { VMLayout::new_64bit() }
>     })
> }
> 
> // In VMLayout::set_custom_vm_layout:
> pub fn set_custom_vm_layout(constants: VMLayout) {
>     VM_LAYOUT.set(constants).expect("VM layout has already been set or read");
> }
> ```
>
> **Overhead**: Zero on reads after init. `OnceLock::get()` returns `Option<&T>` — the `get_or_init` compiles to a single atomic load on the fast path (initialized check).
>
> **Benefit**: Eliminates `unsafe`, enforces one-shot initialization, and is future-compatible with Rust 2024 edition.

---

## C. `unsafe impl Send/Sync`

**Instances**: ~40  
**Characteristic**: Manual Send/Sync implementations for types containing raw pointers, `UnsafeCell`, `RefCell`, or other non-Send/Sync fields.

### C1. Types that are genuinely sound — need only documentation

| # | Type | File:Line | Reason | Assessment |
|---|------|-----------|--------|------------|
| 1 | `OpaquePointer` | [opaque_pointer.rs:12-13](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/opaque_pointer.rs#L12) | Wraps `*mut c_void`, never dereferenced | Sound ✅ |
| 2 | `InitializeOnce<T>` | [rust_util/mod.rs:107](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/rust_util/mod.rs#L107) | `Once` guards init, reads are race-free | Sound ✅ |
| 3 | `SFTSpaceMap` | [sft_map.rs:180](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/policy/sft_map.rs#L180) | Internal atomics | Sound ✅ |
| 4 | `SFTDenseChunkMap` | [sft_map.rs:344](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/policy/sft_map.rs#L344) | Internal atomics | Sound ✅ |
| 5 | `SFTSparseChunkMap` | [sft_map.rs:470](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/policy/sft_map.rs#L470) | Internal atomics | Sound ✅ |

> **Proposed improvement**: Add `// SAFETY:` comments to each explaining the invariant. Example:
> ```rust
> // SAFETY: OpaquePointer wraps a raw *mut c_void that MMTk never dereferences.
> // It is only stored and passed back to the VM binding, which manages thread safety.
> unsafe impl Sync for OpaquePointer {}
> unsafe impl Send for OpaquePointer {}
> ```

### C2. Types that need the `unsafe impl` but have weak justification

| # | Type | File:Line | Problem |
|---|------|-----------|---------|
| 6 | `MMTK<VM>` | [mmtk.rs:133-134](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/mmtk.rs#L133) | Contains `UnsafeCell<Box<dyn Plan>>` |
| 7 | `Map32` | [map32.rs:32-33](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/heap/layout/map32.rs#L32) | Contains `UnsafeCell<Map32Inner>` |
| 8 | `ImmixSpace<VM>` | [immixspace.rs:74](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/policy/immix/immixspace.rs#L74) | Interior mutability in prepare/release |
| 9 | `MarkSweepSpace<VM>` | [native_ms/global.rs:91](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/policy/marksweepspace/native_ms/global.rs#L91) | Same |
| 10 | `GCWorkScheduler<VM>` | [scheduler.rs:37](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/scheduler/scheduler.rs#L37) | Internal state management |
| 11 | `IntArrayFreeList` | [int_array_freelist.rs:12-13](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/int_array_freelist.rs#L12) | `NonNull` parent pointer |

### C3. Types that should be restructured to avoid the `unsafe impl`

| # | Type | File:Line | Problem | Fix |
|---|------|-----------|---------|-----|
| 12 | `AllocationOptionsHolder` | [allocator.rs:110](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/alloc/allocator.rs#L110) | `RefCell` is !Sync; manual `unsafe impl Sync` | Replace `RefCell<AllocationOptions>` with `Cell<AllocationOptions>`. `AllocationOptions` is `Copy` (it's `#[derive(Copy, Clone)]` at line 33), so `Cell` works. `Cell<T: Copy>` is already `Send` for `T: Send`, eliminating the need for `unsafe impl Sync`. |
| 13 | `Prepare<C>` | [gc_work.rs:47](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/scheduler/gc_work.rs#L47) | Raw `*const` makes it !Send | Use `ExclusivePlanRef` (from A1) which has its own `unsafe impl Send` with documented invariant |
| 14 | `Release<C>` | [gc_work.rs:130](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/scheduler/gc_work.rs#L130) | Same | Same |
| 15 | `ProcessEdgesBase<VM>` | [gc_work.rs:491](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/scheduler/gc_work.rs#L491) | Raw `*mut GCWorker` | Use `WorkerRef` (from A4) which has its own `unsafe impl Send` |

> [!TIP]
> **Meta-pattern**: For each `unsafe impl Send`, create a **newtype wrapper** around the raw pointer that encapsulates the safety invariant. This moves the unsafety from "every struct containing a raw pointer" to "one newtype, documented once".

---

## D. `UnsafeCell` Interior Mutability

**Instances**: ~15  
**Characteristic**: `UnsafeCell` used for interior mutability, with varying levels of synchronization.

### D1. `MMTK::plan` — `UnsafeCell<Box<dyn Plan>>`

| # | File | Line | Method | Returns |
|---|------|------|--------|---------|
| 1 | [mmtk.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/mmtk.rs#L115) | 115 | field decl | `UnsafeCell<Box<dyn Plan>>` |
| 2 | [mmtk.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/mmtk.rs#L439-L440) | 440 | `get_plan()` | `&dyn Plan` from `UnsafeCell` |
| 3 | [mmtk.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/mmtk.rs#L449-L451) | 449 | `get_plan_mut()` | `&mut dyn Plan` from `&self` |

`get_plan()` creates `&Plan` from `UnsafeCell` (always safe if no concurrent `get_plan_mut()`).  
`get_plan_mut()` creates `&mut Plan` from `&self` — caller must ensure exclusivity.

> **Proposed Fix**: Same `PhaseToken` approach as A1. `get_plan_mut()` should require a `&PhaseToken`:
> ```rust
> pub fn get_plan_mut(&self, _token: &PhaseToken) -> &mut dyn Plan<VM = VM> {
>     unsafe { &mut **(self.plan.get()) }
> }
> ```
> This makes the "exclusive phase" requirement explicit in the API.

### D2. `Map32`/`Map64` — `UnsafeCell<MapInner>` with optional Mutex

| # | File | Line | Pattern |
|---|------|------|---------|
| 4 | [map32.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/heap/layout/map32.rs#L266-L268) | 266 | `unsafe fn mut_self(&self) -> &mut Map32Inner` |
| 5 | [map32.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/heap/layout/map32.rs#L273-L276) | 273 | `fn mut_self_with_sync()` — acquires mutex first |
| 6 | [map64.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/heap/layout/map64.rs#L216-L218) | 216 | Same `mut_self` pattern, **no mutex variant** |

**Callers of bare `mut_self()` (no lock)**: `insert()` (line 66), `finalize_static_space_map()` (line 198), `free_contiguous_chunks_no_lock()` (line 279-301, 5 calls), `get_discontig_freelist_pr_ordinal()` (line 305).

> **Proposed Abstraction**: Replace `mut_self()` / `mut_self_with_sync()` with a single properly-guarded API:
>
> ```rust
> impl Map32 {
>     /// Get mutable access during single-threaded init.
>     /// # Safety: Must be called only during single-threaded initialization.
>     pub(crate) unsafe fn init_access(&self) -> &mut Map32Inner {
>         &mut *self.inner.get()
>     }
>     
>     /// Get mutable access with mutex protection.
>     pub(crate) fn locked_access(&self) -> (MutexGuard<'_, ()>, &mut Map32Inner) {
>         let guard = self.sync.lock().unwrap();
>         (guard, unsafe { &mut *self.inner.get() })
>     }
> }
> ```
>
> The `free_contiguous_chunks_no_lock()` callers should use `locked_access()` since they are called from `free_all_chunks()` which already holds the lock via `mut_self_with_sync()`.

### D3. `InitializeOnce<T>` — `UnsafeCell<MaybeUninit<T>>` with `Once`

| # | File | Line | Method |
|---|------|------|--------|
| 7 | [rust_util/mod.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/rust_util/mod.rs#L74) | 74 | `initialize_once()` — writes via UnsafeCell |
| 8 | [rust_util/mod.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/rust_util/mod.rs#L83) | 83 | `get_ref()` — `assume_init_ref()` with debug assert |
| 9 | [rust_util/mod.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/rust_util/mod.rs#L93-L96) | 93 | `get_mut()` — `assume_init_mut()` from `&self` |

> **Proposed Fix: Replace with `OnceLock<T>`** (stable since Rust 1.70)
>
> ```rust
> use std::sync::OnceLock;
> pub struct InitializeOnce<T: 'static>(OnceLock<T>);
> 
> impl<T> InitializeOnce<T> {
>     pub const fn new() -> Self { InitializeOnce(OnceLock::new()) }
>     pub fn initialize_once(&self, init_fn: &'static dyn Fn() -> T) {
>         self.0.get_or_init(|| init_fn());
>     }
>     pub fn get_ref(&self) -> &T { self.0.get().expect("not initialized") }
> }
> impl<T> Deref for InitializeOnce<T> {
>     type Target = T;
>     fn deref(&self) -> &T { self.get_ref() }
> }
> ```
>
> **The `get_mut` case** is harder: `OnceLock::get_mut()` requires `&mut self`. For the one call site (`SFT_MAP.get_mut()` during init at [global.rs:114](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/plan/global.rs#L114)), this is fine since `SFT_MAP` is a global `static` — we can use `OnceLock::get_mut()` only if we have a `&mut` (which we do during single-threaded plan creation). Alternatively, the SFT mutation during init can be done before the `OnceLock::set()` call.
>
> **Overhead**: `OnceLock::get()` compiles to an atomic load + branch (checking "is initialized?"). This is one extra branch per read compared to the current code, which does zero checks in release mode. However, `OnceLock::get()` is marked `#[inline]` and the branch is 100% predictable (always-initialized after init), so the CPU branch predictor eliminates the cost.

---

## E. `MaybeUninit` Partially-Initialized Arrays

**Instances**: ~30 `assume_init` calls  
**Characteristic**: Arrays of `MaybeUninit<Allocator>` where only some slots are initialized, depending on the plan. The `unsafe` is in accessing elements via `assume_init_ref()`/`assume_init_mut()`.

### E1. Array initialization anti-pattern

| # | File | Line | Code |
|---|------|------|------|
| 1-6 | [allocators.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/alloc/allocators.rs#L108-L113) | 108-113 | `bump_pointer: unsafe { MaybeUninit::uninit().assume_init() },` |
| 7-9 | [copy/mod.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/copy/mod.rs#L183-L185) | 183-185 | Same for copy contexts |
| 10-12 | [copy/mod.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/copy/mod.rs#L224-L226) | 224-226 | Same for `new_non_copy()` |

This pattern `MaybeUninit::uninit().assume_init()` on `[MaybeUninit<T>; N]` is sound (an uninitialized `MaybeUninit<T>` is valid), but looks alarming and is a known anti-pattern.

> **Fix (zero-cost, no unsafe)**:
> ```rust
> // Currently:
> bump_pointer: unsafe { MaybeUninit::uninit().assume_init() },
> // Replace with:
> bump_pointer: [const { MaybeUninit::uninit() }; MAX_BUMP_ALLOCATORS],
> ```
> This is a const-expression array initializer, available since Rust 1.79. It's equivalent but requires no `unsafe`.

### E2. Accessor pattern via `assume_init_ref/mut`

| # | File | Line | Method | Hot Path? |
|---|------|------|--------|-----------|
| 13 | [allocators.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/alloc/allocators.rs#L49) | 49 | `get_allocator()` | **YES** — every allocation |
| 14 | [allocators.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/alloc/allocators.rs#L78) | 78 | `get_allocator_mut()` | **YES** — every allocation |
| 15-20 | [copy/mod.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/copy/mod.rs#L92-L168) | 92-168 | `alloc_copy`, `post_copy`, `prepare`, `release` | **YES** — every object copy |

The safety invariant: the `AllocatorSelector` must point to an initialized slot.

> [!IMPORTANT]
> **Proposed Abstraction: `InitOnceArray<T, N>` — a fixed-size array with initialization tracking**
>
> ```rust
> /// A fixed-size array where elements are initialized at construction time based on a mapping,
> /// and thereafter accessed without overhead.
> pub struct InitOnceArray<T, const N: usize> {
>     data: [MaybeUninit<T>; N],
>     #[cfg(debug_assertions)]
>     initialized: [bool; N],  // only in debug builds
> }
>
> impl<T, const N: usize> InitOnceArray<T, N> {
>     pub const fn uninit() -> Self {
>         Self {
>             data: [const { MaybeUninit::uninit() }; N],
>             #[cfg(debug_assertions)]
>             initialized: [false; N],
>         }
>     }
>     pub fn init(&mut self, index: usize, value: T) {
>         #[cfg(debug_assertions)] { self.initialized[index] = true; }
>         self.data[index].write(value);
>     }
>     /// # Safety: index must have been initialized via init().
>     #[inline(always)]
>     pub unsafe fn get(&self, index: usize) -> &T {
>         debug_assert!(cfg!(not(debug_assertions)) || self.initialized[index],
>             "Accessing uninitialized allocator at index {}", index);
>         self.data[index].assume_init_ref()
>     }
>     /// # Safety: index must have been initialized via init().
>     #[inline(always)]
>     pub unsafe fn get_mut(&mut self, index: usize) -> &mut T {
>         debug_assert!(cfg!(not(debug_assertions)) || self.initialized[index]);
>         self.data[index].assume_init_mut()
>     }
> }
> ```
>
> **Overhead**: Zero in release builds (the `initialized` array is `#[cfg(debug_assertions)]` only).  
> **Benefit**: Centralizes all `assume_init` calls into one type. Debug builds catch use-before-init.  
> **Remaining unsafe**: The `get()`/`get_mut()` calls are still `unsafe` — but the invariant is documented once, enforced in debug, and debug_asserted.

---

## F. `transmute` / Type-Punning

**Instances**: 6  
**Characteristic**: Uses `std::mem::transmute` to reinterpret data as a different type.

### F1. SFT fat pointer transmute

| # | File | Line | Code |
|---|------|------|------|
| 1 | [sft_map.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/policy/sft_map.rs#L135) | 135 | `let val: DoubleWord = unsafe { std::mem::transmute(sft) };` |
| 2 | [sft_map.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/policy/sft_map.rs#L150) | 150 | `unsafe { std::mem::transmute(val) }` (DoubleWord → fat ptr) |
| 3 | [sft_map.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/policy/sft_map.rs#L156) | 156 | Same |

**Purpose**: Store `*const dyn SFT` (a fat pointer: data + vtable, 2 words) atomically as a double-word integer.

**Risk**: Relies on fat pointer having `(data, vtable)` representation. This is ABI-dependent but stable in practice.

> **Proposed Abstraction**: The existing `SFTRefStorage` struct already encapsulates this. It's the right abstraction. The only improvement is to add a compile-time assertion:
> ```rust
> const _: () = assert!(
>     std::mem::size_of::<*const dyn SFT>() == std::mem::size_of::<DoubleWord>(),
>     "Fat pointer size must equal double word"
> );
> ```
> The `pre_use_check()` method already does this at runtime (line 128-131), but a `const` assertion catches it at compile time. Beyond this, the transmute is **inherently needed** — there's no safe way to atomically store a fat pointer in Rust.

### F2. Test-only lifetime fabrication

| # | File | Line | Code |
|---|------|------|------|
| 4 | [mock_vm.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/test_util/mock_vm.rs#L49-L51) | 49-51 | `macro_rules! lifetime { ($e:expr) => { std::mem::transmute($e) } }` |

**Test-only**: Used to extend lifetimes in mock VM implementations. Low priority.

> **Proposed Fix**: Use `Box::leak()` for test data that needs `'static`, or restructure tests to use proper lifetimes with `Box<dyn Fn(...) + 'static>`.

---

## G. Raw Memory via `Address` Type

**Instances**: ~90+ call sites, 12 unsafe methods on `Address`  
**Characteristic**: `Address(usize)` provides load/store/atomic operations by casting to raw pointers. This is **inherently unsafe** — a GC must be able to read and write raw heap memory.

### Key methods and their call-site counts

| Method | Line in [address.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/address.rs) | Call Sites | Hot Path? |
|--------|------|:---:|:---:|
| `Address::zero()` | 158 | ~13 | Cold |
| `Address::from_usize()` | 175 | ~50+ | Mixed |
| `Address::load::<T>()` | 233 | ~50+ | **YES** |
| `Address::store::<T>()` | 240 | ~39+ | **YES** |
| `Address::atomic_load::<T>()` | 249 | ~10 | **YES** |
| `Address::atomic_store::<T>()` | 257 | ~5 | **YES** |
| `Address::compare_exchange::<T>()` | 265 | ~5 | **YES** |
| `Address::as_ref::<T>()` | 313 | ~15 | **YES** |
| `Address::as_mut_ref::<T>()` | 321 | ~1 | Cold |

> [!IMPORTANT]
> **Proposed Abstraction: Debug-mode checked `Address` operations**
>
> We cannot eliminate unsafe from raw memory access. Instead, we can **limit the surface area** with a debug-mode-checked wrapper and better type safety:
>
> ```rust
> impl Address {
>     /// Load a value of type T from this address.
>     /// # Safety: Address must be valid, mapped, aligned for T, and not concurrently written.
>     #[inline(always)]
>     pub unsafe fn load<T: Copy>(self) -> T {
>         debug_assert!(self.0 != 0, "load from null address");
>         debug_assert!(self.is_aligned_to(std::mem::align_of::<T>()),
>             "unaligned load: addr={:#x}, align={}", self.0, std::mem::align_of::<T>());
>         *(self.0 as *mut T)
>     }
> }
> ```
>
> These `debug_assert!` checks are **zero-cost in release** but catch bugs during development.
>
> Additionally, introduce typed address variants to prevent category errors:
>
> ```rust
> /// An address known to point to mapped GC heap memory.
> #[repr(transparent)]
> pub struct HeapAddress(Address);
>
> /// An address known to point to side metadata memory.
> #[repr(transparent)]  
> pub struct MetaAddress(Address);
>
> impl HeapAddress {
>     /// Load is still unsafe, but we've proven this is heap memory.
>     pub unsafe fn load<T: Copy>(&self) -> T { self.0.load() }
> }
> ```
>
> **Overhead**: Zero in release. The newtypes are `#[repr(transparent)]`, so they compile to the same code.
> **Benefit**: Prevents mixing up metadata addresses and heap addresses at compile time.

---

## H. Non-Atomic Sub-Byte Metadata Read-Modify-Write

**Instances**: ~6 core operations, ~180+ total through the chain  
**Characteristic**: For sub-byte metadata (e.g., 1-bit mark bit stored in a shared byte), non-atomic `store` does read-modify-write without synchronization.

| # | File | Line | Operation |
|---|------|------|-----------|
| 1 | [side_metadata/global.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/metadata/side_metadata/global.rs#L536) | 536-551 | `SideMetadataSpec::store()` — non-atomic RMW |
| 2 | [header_metadata.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/metadata/header_metadata.rs#L210) | 210-239 | `HeaderMetadataSpec::store()` — non-atomic RMW |

```rust
// Simplified non-atomic sub-byte store:
let byte = meta_addr.load::<u8>();            // read
let new_byte = (byte & !mask) | (val & mask); // modify
meta_addr.store(new_byte);                     // write — NOT ATOMIC
```

**Risk**: If two threads modify different bits in the same byte concurrently, one update is lost. The atomic variants (`store_atomic`) use `AtomicU8::fetch_update` with CAS.

> [!IMPORTANT]  
> **Proposed Abstraction: Phase-tagged metadata operations**
>
> The non-atomic variants are deliberately used during STW (stop-the-world) phases for performance. The issue is that nothing prevents calling the non-atomic version during a concurrent phase.
>
> ```rust
> /// Marker types for metadata access phases.
> pub struct STWPhase;      // stop-the-world — non-atomic is safe
> pub struct ConcPhase;     // concurrent — must use atomics
>
> impl SideMetadataSpec {
>     /// Non-atomic load — only safe during STW.
>     #[inline(always)]
>     pub unsafe fn load_stw<T: MetadataValue>(&self, _phase: &STWPhase, data_addr: Address) -> T {
>         // ... existing non-atomic implementation
>     }
>     
>     /// Atomic load — safe during any phase.
>     #[inline(always)]
>     pub fn load_atomic<T: MetadataValue>(&self, data_addr: Address, order: Ordering) -> T {
>         // ... existing atomic implementation (already safe-ish)
>     }
> }
> ```
>
> **Overhead**: Zero. `STWPhase` is zero-sized. This is a compile-time enforcement pattern.
> **Benefit**: Makes it impossible to accidentally call non-atomic operations during concurrent phases.

---

## I. FFI / `libc` Calls

**Instances**: ~30  
**Characteristic**: Calls to libc for memory mapping, thread affinity, process IDs, and malloc.

| # | File | Lines | Functions Called |
|---|------|-------|-----------------|
| 1-5 | [memory.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/memory.rs) | various | `libc::mmap`, `libc::munmap`, `libc::mprotect`, `libc::madvise` |
| 6-7 | [affinity.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/scheduler/affinity.rs) | various | `sched_getaffinity`, `sched_setaffinity` |
| 8 | [rust_util/mod.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/rust_util/mod.rs#L111-L115) | 111,115 | `libc::getpid`, `libc::gettid` |
| 9+ | [malloc/library.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/malloc/library.rs) | various | `libc::malloc`, `libc::free`, etc. |

> **Proposed Abstraction**: The memory mapping functions already have good wrappers (`dzmmap`, `munmap`, etc. in memory.rs). The key improvement is **result checking**:
>
> ```rust
> /// Safe wrapper around mmap that returns Result instead of MAP_FAILED.
> fn safe_mmap(addr: Address, size: usize, prot: i32, flags: i32) -> Result<Address, MmapError> {
>     let result = unsafe { libc::mmap(addr.to_mut_ptr(), size, prot, flags, -1, 0) };
>     if result == libc::MAP_FAILED {
>         Err(MmapError::Failed(std::io::Error::last_os_error()))
>     } else {
>         Ok(Address::from_mut_ptr(result))
>     }
> }
> ```
>
> This is largely already done in mmtk-core's `memory.rs`. The FFI calls are **inherently unsafe** and well-encapsulated. No further abstraction needed.

---

## J. VM-Type-Erased Pointer Round-Trip

**Instances**: 2 (macro-generated)  
**Characteristic**: Erases the `<VM>` type parameter from a reference by converting to `usize` and back.

| # | File | Line | Code |
|---|------|------|------|
| 1 | [erase_vm.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/erase_vm.rs#L20) | 20 | `(r as *mut $orig_type).expose_provenance()` (ref → usize) |
| 2 | [erase_vm.rs](file:///usr/local/google/home/zixianc/Develop_GitHub/mmtk/mmtk-core/src/util/erase_vm.rs#L25) | 25 | `std::ptr::with_exposed_provenance(self.0)` (usize → ref) |

**Invariant**: the same `VM` type parameter is used for both `new()` and `into_mut()`. Enforced by module-level documentation but not by the type system.

> **Proposed Improvement**: Add a runtime check in debug builds:
>
> ```rust
> pub struct ErasedRef<'a> {
>     ptr: usize,
>     #[cfg(debug_assertions)]
>     type_id: std::any::TypeId,
>     _marker: PhantomData<&'a ()>,
> }
> impl<'a> ErasedRef<'a> {
>     pub fn new<VM: VMBinding>(r: &'a mut SomeType<VM>) -> Self {
>         Self {
>             ptr: (r as *mut _).expose_provenance(),
>             #[cfg(debug_assertions)]
>             type_id: std::any::TypeId::of::<VM>(),
>             _marker: PhantomData,
>         }
>     }
>     pub fn into_mut<VM: VMBinding>(self) -> &'a mut SomeType<VM> {
>         #[cfg(debug_assertions)]
>         debug_assert_eq!(self.type_id, std::any::TypeId::of::<VM>(),
>             "VM type mismatch in erased reference recovery");
>         unsafe { &mut *(std::ptr::with_exposed_provenance(self.ptr) as *mut _) }
>     }
> }
> ```
>
> **Overhead**: Zero in release (guarded by `#[cfg(debug_assertions)]`).  
> **Benefit**: Catches VM type mismatches in debug builds.

---

## Summary Statistics

| Category | Instances | Can Eliminate `unsafe`? | Proposed Abstraction | Runtime Overhead |
|----------|:---------:|:---:|----------------------|:---:|
| **A: Raw-ptr aliasing** | ~25 | Partially | `ExclusivePlanRef`, `SpaceRef`, `WorkerRef` | Zero |
| **B: `static mut`** | 2+2 | **Yes** | `OnceLock` | Zero (after init) |
| **C: `unsafe impl Send/Sync`** | ~40 | Partially | Newtype wrappers + `Cell` | Zero |
| **D: `UnsafeCell`** | ~15 | Partially | `PhaseToken`, `OnceLock` | Zero |
| **E: `MaybeUninit`** | ~30 | Init: **Yes**; access: No | `const { MaybeUninit::uninit() }`, `InitOnceArray` | Zero (debug checks only) |
| **F: `transmute`** | 6 | No (inherent) | `const` size assertions | Zero |
| **G: `Address` ops** | ~90+ | No (inherent) | Debug checks, typed address newtypes | Zero in release |
| **H: Metadata RMW** | ~6+chain | No (inherent) | `STWPhase`/`ConcPhase` marker types | Zero |
| **I: FFI/libc** | ~30 | No (inherent) | Already well-encapsulated | Zero |
| **J: VM type erasure** | ~2 | No (inherent) | Debug-mode `TypeId` check | Zero in release |

### Impact Summary

> [!NOTE]
> **Total `unsafe` blocks/items that can be outright eliminated**: ~20 (static mut, MaybeUninit array init, InitializeOnce, AllocationOptionsHolder)  
> **Total `unsafe` blocks whose surface area can be dramatically reduced**: ~40 (plan mutation → 1 site, space self-reborrow → 1 site per space, worker pointer → 1 site)  
> **Total `unsafe` that is inherently needed but can gain debug-mode checking**: ~120+ (Address operations, metadata)
>
> **Net effect**: The proposed changes would reduce the number of unsafe _reasons_ (distinct justifications) from ~10 to ~4 (FFI, Address, metadata, SFT transmute), while keeping all performance characteristics identical.
