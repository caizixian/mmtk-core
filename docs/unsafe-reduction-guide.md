# Unsafe Reduction Guide for mmtk-core

This document captures refactoring lessons and coding patterns for reducing `unsafe` in mmtk-core
while preserving correctness, performance, and API compatibility.

## General Principles

1. **Unsafe should encode invariants, not be a convenience shortcut.** If `unsafe` exists because
   "we know this is initialized", replace the pattern with one where the type system guarantees
   initialization (e.g., `Option<T>` instead of `MaybeUninit<T>`).

2. **Centralize, don't scatter.** Prefer one `unsafe` abstraction (e.g., `MetadataSlot`) over
   dozens of scattered raw pointer dereferences.

3. **Never silently change semantics.** Don't make `unsafe fn store()` safe by secretly switching
   from non-atomic to atomic internally. If the function was non-atomic, keep it non-atomic or
   document the change explicitly.

4. **Public API changes belong in separate PRs.** Don't mix internal unsafe cleanup with trait
   signature changes (e.g., `ProcessEdgesWork` worker threading) — these are separate concerns
   with different review requirements.

5. **`cargo check` must pass at every commit.** Each commit should be a self-contained, reviewable
   unit.

---

## Pattern Catalog

### Pattern 1: Union → Enum

**Before:** `union` with unsafe field access, manual `PartialEq`/`Hash` impls
```rust
pub union SideMetadataOffset {
    addr: Address,
    rel_offset: usize,
}
// Every access requires: unsafe { self.offset.addr }
```

**After:** `enum` with derived traits
```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum SideMetadataOffset {
    Addr(Address),
    Rel(usize),
}
// Access via match: match self.offset { SideMetadataOffset::Addr(a) => a, ... }
```

**When to apply:** Whenever a union is used as a tagged union with a discriminant tracked elsewhere.

---

### Pattern 2: MaybeUninit Arrays → Option Arrays

**Before:** `MaybeUninit` with `assume_init()` — UB if accessed before initialization
```rust
struct Allocators {
    bump_pointer: [MaybeUninit<BumpAllocator>; N],
}
// Access: unsafe { self.bump_pointer[i].assume_init_ref() }
```

**After:** `Option<T>` with runtime checks — panics on misuse, never UB
```rust
struct Allocators {
    bump_pointer: [Option<BumpAllocator>; N],
}
// Access: self.bump_pointer[i].as_ref().expect("not initialized")
// Init:   [const { None }; N]
```

**When to apply:** Whenever the array is small (≤16 elements) and accessed infrequently relative
to the hot path. The `Option` overhead (size + branch) is negligible for allocator selection which
happens once per allocation.

**Caveat:** Do NOT apply when `repr(C)` layout is required for FFI, or when the type is very large
and the `Option` discriminant adds unacceptable memory overhead.

---

### Pattern 3: UnsafeCell + Manual Sync → Mutex/RwLock

**Before:** Interior mutability with manual locking discipline
```rust
struct Map32 {
    sync: Mutex<()>,
    inner: UnsafeCell<Map32Inner>,
}
unsafe impl Send for Map32 {}
unsafe impl Sync for Map32 {}
// Access: let guard = self.sync.lock(); unsafe { &mut *self.inner.get() }
```

**After:** `Mutex<Inner>` — the lock owns the data
```rust
struct Map32 {
    inner: Mutex<Map32Inner>,
}
// Access: let inner = self.inner.lock().unwrap();
```

**When to apply:** Whenever a `Mutex<()>` + `UnsafeCell<T>` pair exists. The Mutex should own the
data it protects. Fields that need lock-free reads can be moved out as `AtomicT`.

---

### Pattern 4: Raw Pointer Type Erasure → `dyn Any`

**Before:** Cast to usize and back — UB if types don't match
```rust
pub struct ErasedRef(usize);
impl ErasedRef {
    pub fn new<VM>(r: &mut Worker<VM>) -> Self { Self(r as *mut _ as usize) }
    pub fn into_mut<VM>(self) -> &mut Worker<VM> {
        unsafe { &mut *(self.0 as *mut Worker<VM>) }
    }
}
```

**After:** `dyn Any` downcast — panics on type mismatch, never UB
```rust
pub struct ErasedRef<'a>(&'a mut dyn std::any::Any);
impl<'a> ErasedRef<'a> {
    pub fn new<VM: 'static>(r: &'a mut Worker<VM>) -> Self { Self(r) }
    pub fn into_mut<VM: 'static>(self) -> &'a mut Worker<VM> {
        self.0.downcast_mut().expect("type mismatch")
    }
}
```

**When to apply:** Whenever usize/pointer type erasure is used to cross generic boundaries.
Requires the erased types to be `'static`.

---

### Pattern 5: `InitializeOnce` with `UnsafeCell` → `OnceLock`

**Before:** Hand-rolled lazy init with `UnsafeCell + Once`
```rust
struct InitializeOnce<T> {
    v: UnsafeCell<MaybeUninit<T>>,
    once: Once,
}
unsafe impl<T> Sync for InitializeOnce<T> {}
```

**After:** `std::sync::OnceLock` — safe, standard library
```rust
struct InitializeOnce<T> {
    lock: OnceLock<T>,
}
// No unsafe impl Sync needed — OnceLock<T: Sync> is Sync
```

**When to apply:** When the value is initialized exactly once and read many times. `OnceLock::get()`
returns `Option<&T>` with a branch, but this is negligible compared to the initialization cost.

**Caveat:** The original `InitializeOnce` had zero-cost reads (no branch in release builds). If
this is on a very hot path, consider keeping the custom implementation or using a flag check.

---

### Pattern 6: SFTMap Trait Safety

**Before:** `unsafe fn update(&self, space: *const dyn SFT, ...)`
```rust
// Raw pointer — caller must ensure validity
unsafe fn update(&self, space: *const (dyn SFT + Sync + 'static), start: Address, bytes: usize);
```

**After:** Safe reference — Rust guarantees validity
```rust
fn update(&self, space: &(dyn SFT + Sync + 'static), start: Address, bytes: usize);
```

**When to apply:** Whenever a raw pointer parameter can be replaced with a reference. This is
only possible when the caller can prove the reference is valid for the required lifetime.

---

### Pattern 7: Stop-the-World Proof Token

**Before:** `unsafe fn get_plan_mut(&self) -> &mut dyn Plan`
```rust
// Caller must manually ensure no concurrent access
pub unsafe fn get_plan_mut(&self) -> &mut dyn Plan { ... }
```

**After:** Proof token — the type system encodes the invariant
```rust
pub struct StwProof(()); // Zero-sized, can only be created during STW
pub fn get_plan_mut(&self, _proof: &StwProof) -> &mut dyn Plan { ... }
```

**When to apply:** When mutation is safe only during specific GC phases (stop-the-world). The
`StwProof` type should only be constructible when the invariant holds (e.g., during GC).

---

### Pattern 8: MaybeUninit Plan Pointer → Parameter Threading

**Before:** Late-initialized plan pointer in `GCTrigger`
```rust
struct GCTrigger {
    plan: MaybeUninit<&'static dyn Plan>,
}
// Must call set_plan() before any use
```

**After:** Pass plan as a parameter
```rust
struct GCTrigger { /* no plan field */ }
impl GCTrigger {
    fn poll(&self, plan: &dyn Plan, ...) -> bool { ... }
}
```

**When to apply:** When a struct stores a reference that's not available at construction time.
Threading it as a parameter is cleaner than late initialization with `MaybeUninit`.

---

### Pattern 9: `unsafe impl Sync` → Derive via Atomic Fields

**Before:** Manual `Sync` impl with non-atomic interior data
```rust
struct SFTSpaceMap { sft: Vec<SFTRefStorage> }
unsafe impl Sync for SFTSpaceMap {}
```

**After:** If `SFTRefStorage` wraps `AtomicPtr` (which is `Sync`), the auto-derive works:
```rust
struct SFTSpaceMap { sft: Vec<SFTRefStorage> }
// No unsafe impl needed — Vec<SFTRefStorage: Sync> is Sync
```

**When to apply:** When all fields are actually `Sync`-safe and the `unsafe impl` was just
working around a missing `Sync` bound on an inner type.

---

### Pattern 10: `derive(Zeroable)` for Zero-Valid Types

**Before:** `unsafe impl Zeroable` with manual audit
```rust
unsafe impl Zeroable for SpaceDescriptor {}
```

**After:** For simple newtypes over integers, `derive` is safe:
```rust
#[derive(Zeroable)]  // or: unsafe impl Zeroable — but verify zero is valid!
struct SpaceDescriptor(usize);
```

**When to apply:** When `all-zeros` is a valid bit pattern for the type. For `#[repr(transparent)]`
wrappers over integer types, this is always true.

---

## Irreducible Unsafe Categories in mmtk-core

Some unsafe is **fundamental** and cannot be eliminated:

1. **FFI calls** — `libc::mmap`, `mprotect`, `munmap`, CPU affinity. These are inherently unsafe.
2. **Raw memory metadata** — Side metadata accesses raw memory at computed addresses. The
   `MetadataSlot` abstraction centralizes this but can't eliminate it.
3. **`Address::load/store`** — Loading/storing values at arbitrary addresses is inherently unsafe.
4. **`SFTRefStorage` transmute** — Atomically storing fat pointers (trait objects) requires
   transmute to/from an integer type for CAS operations.
5. **Scheduler worker pointer** — The `ProcessEdgesWork` architecture stores a `*mut GCWorker`
   for performance. Eliminating this requires an API redesign.
6. **`IntArrayFreeList` parent-child sharing** — Parent/child freelists share data via `NonNull`,
   requiring `unsafe impl Send/Sync`.
