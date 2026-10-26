# Programming Languages, Formal Verification & Rust Type-System Literature Review for Eliminating Cross-Boundary `unsafe` in `mmtk-core`

**Author**: `@pl-lit` (PL, Formal Verification & Rust Type-System Literature Researcher)  
**Target Codebase**: `mmtk-core` (`edition = "2021"`, `rust-version = "1.84"`, `bytemuck = "1.14"` already in `Cargo.toml`)  
**Companion Design Doc**: `scratch/cross_boundary_design.md` (`@lit-arch`)

---

## Executive Summary

An initial architecture pass (`scratch/cross_boundary_design.md`, v1) surveyed six well-known Rust cell and OS patterns (`GhostCell`, `qcell`, `Asterinas`, `Tock`, `Theseus`, `gc-arena`) and designed sound wrappers for **temporal & thread-local interior mutability** (`StwCell`, `WorkerLocal`, `LeakedPlan`, `OnceLock` singletons, `ErasedVmMutRef`). While valuable for the ~130 `UnsafeCell` / `static mut` sites in `plan`, `scheduler`, and `heap`, that lens leaves untouched **over 350 remaining `unsafe` sites** across `src/util/metadata/**` (229 post-Stage-1), `src/policy/**` (~105), `src/util/**` (93), `src/util/heap/**` (24), and `src/util/alloc/**` (~25).

Why do those 350+ sites exist? Inspecting `src/util/address.rs:25` reveals the historical root cause: `mmtk-core`'s foundational abstractions descend directly from **`org.vmmagic`** (Frampton, Blackburn, Cheng, Garner, Grove, Moss & Salishev, *VEE 2009*) via the initial Rust 1.x port of MMTk (Lin, Blackburn, Hosking & Norrish, *ISMM 2016*). In Java (`JikesRVM`), `Address` had to be an unrefined word (`Address(usize)`) and metadata descriptors had to be plain integer records because the JVM lacked zero-cost affine types, const generics, lifetime branding, phantom proof witnesses, and bit-validity traits. Carried verbatim into Rust, unrefined `Address`, unvalidated `SideMetadataSpec` / `HeaderMetadataSpec` structs, raw `usize`-encoded back-pointers, and unchecked slice/offset arithmetic force downstream modules (`policy`, `alloc`, `heap`, `plan`) to repeatedly re-assert invariants via `unsafe` blocks or `debug_assert!`s stripped in release builds.

This review synthesizes **six major research families** across **PLDI, POPL, OOPSLA, ICFP, ISMM, VEE, SOSP, OSDI, USENIX ATC, CAV**, and the Rust verification/systems ecosystem:

1. **Refinement Types, Liquid Types & Ghosts of Departed Proofs (GDP)**: `Flux` (*PLDI 2023*), `TickTock` (*SOSP 2025*), `RefinedRust` (*PLDI 2024*), `RefinedC` (*PLDI 2021*), `CN` (*POPL 2023*), Noonan's *Ghosts of Departed Proofs* (*Haskell 2018*), Kiselyov & Shan (*PLPV 2006*), *"Parse, don't validate"*, and Rust 1.84 `const {}` refinement witnesses.
2. **Linear Ghost State, Separation Logic, Pointer Provenance & Verified Allocators**: `Verus` (*OOPSLA 2023*, *SOSP 2024*), Verified Mimalloc (`PointsToRaw` spatial splitting), `VeriSMo` (*OSDI 2024*), `Atmosphere` (*SOSP 2025*), `RustBelt` (*POPL 2018*), `Stacked Borrows` (*POPL 2020*), `Tree Borrows` (*PLDI 2025*), Rust 1.84 *Strict Provenance*, `Creusot` (*ICFP 2022*), `Aeneas` (*ICFP 2022*), `Prusti` (*OOPSLA 2019*), `Gillian-Rust` (*PLDI 2024*), and `Kani`.
3. **Generative Branding Beyond Cells & Zero-`unsafe` Bounds/Arena Indexing**: Launchbury & Peyton Jones (*PLDI 1994*), `GhostCell` (*ICFP 2021*), `bluss/indexing` (`Container<'id>` + `Index<'id>`/`Range<'id>`), `qcell`, `generativity`, const-bounded finite ordinals `Fin<const N: usize>`, and `slotmap` / `rustc_index`.
4. **Bit-Validity Type Systems & Safe Raw-Memory/MMIO Frameworks**: Rust *Project Safe Transmute*, `zerocopy`, `bytemuck` 1.14 (`Zeroable`, `NoUninit`, `AnyBitPattern`, `Pod`, `CheckedBitPattern`, `Contiguous`), `rust-vmm`'s `vm-memory` (`VolatileSlice`, `AtomicAccess`), and Rust-for-Linux (`Opaque<T>`, `LockedBy`, `pin-init`).
5. **Typestate Protocols, Affine-to-Linear Encodings & Minimal-TCB OS Architectures**: Strom & Yemini (*TSE 1986*), Aldrich et al. (*OOPSLA 2009*), linear CPS/receipt tokens, `Asterinas` (*ATC 2025*), `Tock` (*SOSP 2017*), `Theseus` (*OSDI 2020*), `RedLeaf` (*OSDI 2020*), `Hubris`, and empirical `unsafe` bug studies (`Rudra`, *SOSP 2021*; Astrauskas et al., *OOPSLA 2020*).
6. **ISMM / PL Verified & Rust Garbage Collectors**: `org.vmmagic` (*VEE 2009*), `Immix` (*PLDI 2008*), MMTk-in-Rust (*ISMM 2016*), `LXR` (*PLDI 2022*), Hawblitzel & Petrank (*POPL 2009*), Gammie, Hosking & Engelhardt (*PLDI 2015*), `CertiGC` (*OOPSLA 2019*), `Alloy` (*OOPSLA/TOPLAS*), `shifgrethor`, `gc-arena`, `snmalloc` (*ISMM 2019*), and Project `Verona` regions (*OOPSLA 2023*).

Applying these lenses directly to `mmtk-core` uncovered **five active soundness / UB bugs** in `master` that cross-folder Stage-2 refactorings should fix immediately, plus **one critical hot-path correction** to `cross_boundary_design.md` v1 Rank 3 (`VM_LAYOUT`), and yielded **seven concrete Rust-1.84 refactoring blueprints** alongside a **three-tier formal verification roadmap (`Kani` -> `Flux` -> `Verus`/`RefinedRust`)**.

---

## 1. Historical & Empirical Diagnosis: From `org.vmmagic` (2009) to Modern Safe Systems Rust

### 1.1 The `org.vmmagic` Legacy in `mmtk-core`
In *Demystifying Magic: High-Level Low-Level Programming* (**VEE 2009**), Frampton, Blackburn et al. formalized `org.vmmagic` for Java-in-Java runtimes (`JikesRVM` + Java MMTk), introducing unboxed intrinsics (`Address`, `ObjectReference`, `Word`, `Offset`, `Extent`) lowered directly by the JIT compiler. Because Java 5 had type-erased generics and heap-boxed objects, any wrapper type around `Address` would incur object allocation and pointer indirection on GC hot paths. Consequently, **every low-level invariant was erased to raw machine words**:
- Alignment (`addr % BYTES_IN_BLOCK == 0`), non-zero niche validity (`addr != 0`), and heap containment (`heap_start <= addr < heap_end`) lived only in assertions or comments.
- Metadata layouts were untyped bit/shift arithmetic descriptors.

When Lin, Blackburn, Hosking & Norrish (*"Rust as a Language for High Performance GC Implementation"*, **ISMM 2016**) demonstrated MMTk Immix in Rust 1.x, Rust still lacked **const generics** (stabilized in 1.51), **`MaybeUninit`** (1.36), **`OnceLock`** (1.70), **inline `const {}` blocks** (1.79), **Strict Provenance** (1.84), and mature **`bytemuck`/`zerocopy`** derives. Porting `org.vmmagic::Address` as `#[repr(transparent)] pub struct Address(usize)` (`src/util/address.rs:25-33`) was pragmatic in 2016 — but in 2026 Rust 1.84, it leaves the type checker blind to almost every spatial, alignment, bit-width, and initialization invariant in MMTk.

### 1.2 Empirical Taxonomies of `unsafe` Rust Usage & Bugs
Two landmark empirical studies explain both where `unsafe` concentrates in systems crates and where subtle UB hides:
- **Astrauskas, Matheja, Poli, Müller & Summers, *"How Do Programmers Use Unsafe Rust?"* (OOPSLA 2020)**: Auditing thousands of crates, they showed that most `unsafe` blocks rely on **simple, local structural or value preconditions** (bounds, non-null, alignment, disjointness, initialization) that authors *believed* were obvious from surrounding code, rather than deep undecidable math. Making those preconditions explicit in types collapses external `unsafe` footprints dramatically.
- **Bae, Kim, Askar, Lim & Kim, *"Rudra: Finding Memory Safety Bugs in Rust at the Ecosystem Scale"* (SOSP 2021, Distinguished Paper)**: Discovered 264 memory-safety CVEs across the Rust ecosystem (including `std`), proving that `unsafe` bugs cluster at **safe/unsafe abstraction boundaries** in three recurring anti-patterns:
  1. **Send/Sync Variance Bugs**: Manual `unsafe impl Send/Sync` without proper bounds on generic parameters or interior non-atomic state (cf. `InitializeOnce<T>` in `src/util/rust_util/mod.rs:124` and `AllocationOptionsHolder` wrapping `RefCell` with `unsafe impl Sync` in `src/util/alloc/allocator.rs:97-105`!).
  2. **Panic / Unvalidated-Input Bypass**: Safe `pub` constructors or structs with `pub` fields allowing arbitrary inputs to reach internal `unsafe` code whose `debug_assert!` guards vanish in `--release` builds (cf. `HeaderMetadataSpec`, `SideMetadataSpec`, and `native_ms::Block::from_aligned_address(Address::ZERO)`!).
  3. **Higher-Order / Uninitialized / Aliasing Lifetime Laundering**: Forging `&'static [T]` or `&mut T` over concurrently or re-entrantly mutated raw memory (cf. `RCArray` in `src/policy/immix/line.rs:266` and `BLOCK_LIST_TABLE` in `src/policy/marksweepspace/native_ms/block_list.rs:163-185`!).

---

## 2. Systematic Literature Review Across Six Research Families

### Family 1: Refinement Types, Liquid Types & Ghosts of Departed Proofs (GDP)

#### 1A. Liquid & SMT-Backed Refinement Types for Rust (`Flux` & `TickTock`)
- **Lehmann, Geller, Vazou & Jhala, *"Flux: Liquid Types for Rust"*, PLDI 2023** (*PACMPL* 7, PLDI, Art. 169):
  - **Core Mechanism**: Extends Rust types with **index refinements** `T[r]` (wiring a Rust runtime value to a pure logical SMT value `r`, e.g., `usize[n]`, `Address[a]`, `RVec<T>[len]`) and **existential refinements** `{v: T[v] | P(v)}` (e.g., `usize{v: v > 0 && v < 64}`, `Address{a: a % 65536 == 0}`).
  - **Ownership + Strong Updates (`&strg T`)**: Classical liquid type systems (Rondon, Kawaguchi & Jhala, *PLDI 2008*) struggled with imperative mutation because aliasing invalidates refinements. Flux exploits Rust's affine ownership and Stacked Borrows discipline to support **strong updates** through mutable references (`&mut` / `&strg`): mutating a `BumpPointer` from `cursor[c]` to `cursor[c + size]` updates its refinement index in-place, checked via Horn-clause verification conditions discharged automatically by Z3.
  - **Bitvector & Detached Specifications**: Recent Flux releases (`flux-rs`) support SMT **bitvector (`bitvec`) refinements** (exact shift/mask/alignment reasoning!) and **detached/extern specs** (`#[cfg_attr(flux, flux::sig(...))]`), allowing standard stable Rust codebases to compile untouched on `rustc 1.84` while running `cargo flux` in CI.
- **Rindisbacher, Johnson, Lehmann, Potyondy, Pannuto, Savage, Stefan & Jhala, *"TickTock: Verified Isolation in a Production Embedded OS"*, SOSP 2025**:
  - **Systems Breakthrough**: Applied **Flux** to verify hardware Memory Protection Unit (MPU) and process isolation invariants in the production **Tock** kernel across ARMv7-M and RISC-V.
  - **Directly Transferable Lesson for MMTk**: Memory management code is dominated by power-of-two region sizes, bitmasks, alignment (`addr & (size - 1) == 0`), sub-region containment (`region_start <= ptr && ptr + len <= region_end`), and bitfield packing — the exact domain where Flux's `int` + `bitvec` refinements caught **two real kernel-compromise bugs** in Tock while **eliminating redundant runtime bounds/alignment checks on critical paths**.

#### 1B. Separation-Logic Refinement Types (`RefinedRust`, `RefinedC`, `CN`)
- **Gäher, Sammler, Jung, Krebbers & Dreyer, *"RefinedRust: A Type System for High-Assurance Verification of Rust Programs"*, PLDI 2024** (building on Sammler et al., *"RefinedC"*, **PLDI 2021**):
  - **Core Mechanism**: A **foundational** refinement type system for Rust mechanized in Rocq (Coq) atop the **Iris** concurrent separation logic (`Caesium` + `Lithium` proof automation). Unlike pure safe-subset verifiers, RefinedRust verifies **both safe and `unsafe` Rust** (raw pointer arithmetic, `UnsafeCell`, custom smart pointers, inline bit-manipulation) inside one unified ownership-and-refinement judgment.
  - **Key Design Insight**: An `unsafe` module is easiest to verify — and hardest to misuse — when its public boundary exports **ownership-carrying refined types** (`AlignedPtr<ALIGN>`, `OwnedChunk`, `ValidSpec`) rather than raw integers accompanied by prose `# Safety` preconditions.
- **Pulte, Dhokia, Kay,igma, Sewell et al., *"CN: Verifying Systems C Code with Separation-Logic Refinement Types"*, POPL 2023**:
  - Verified the Android **pKVM** hypervisor buddy/page allocator by pairing separation-logic spatial resources (`Owned<T>(p)`) with arithmetic refinements over buddy tree indices and page-table levels.

#### 1C. Ghosts of Departed Proofs (GDP) & Zero-SMT Refinement Newtypes in Vanilla Rust
- **Noonan, *"Ghosts of Departed Proofs (Functional Pearl)"*, Haskell Symposium 2018**; building on **Kiselyov & Shan, *"Lightweight Static Capabilities"*, PLPV 2006**:
  - **Core Mechanism**: Encodes **arbitrary unary and relational predicates** directly in a polymorphic host language **without an external SMT solver**:
    1. **Named Values (`Named<N, T>`)**: Rank-2 existential lifetimes (`'n`) attach a unique type-level name `'n` to a runtime value `x: Named<'n, Address>` (`PhantomData<fn(&'n ()) -> &'n ()>`).
    2. **Zero-Sized Proof Combinators (`Proof<P>`)**: A private-constructor ZST `Proof<P>(PhantomData<P>)` represents a departed runtime check or inductive proof rule (e.g., `IsMapped<'ctx, 'addr>`, `InBounds<'idx, 'len>`, `Aligned<LOG_ALIGN>`).
    3. **Trusted Axiom Basis**: Only tiny, auditable smart constructors (`check_mapped`, `align_down`) can forge `Proof<P>`; safe functions require `Proof<P>` by value/copy and compile down to **zero bytes and zero instructions**.
- **King, *"Parse, don't validate"* (2019)** & Rust Refinement Ecosystems (`nutype`, `refined`, `deranged`, Rust RFC 3352 pattern types):
  - Instead of `fn check_spec(s: &SideMetadataSpec)` returning `()` (discarding the proof so every caller must `debug_assert!` or `unsafe` again), a fallible/const parser `ValidSpec::new(raw) -> ValidSpec` **embeds the invariant in the return type**.
  - **Rust 1.84 Superpower (`inline_const`, stabilized in 1.79)**: Combined with `const` generics, Rust 1.84 supports compile-time proof obligations via `const { assert!(BITS > 0 && BITS <= 64 && BITS.is_power_of_two()) }` inside generic constructors — turning invalid specializations into **hard compile-time errors** with zero runtime cost!

---

### Family 2: Linear Ghost State, Separation Logic, Pointer Models & Verified Allocators

#### 2A. `Verus` Linear Ghost Permissions & Verified Mimalloc (`PointsToRaw`)
- **Lattuada, Hance, Cho, Brun, Subasinghe, Zhou, Howell, Parno & Hawblitzel, *"Verus: Verifying Rust Programs using Linear Ghost Types"*, OOPSLA 2023** (Distinguished Paper); and **Lattuada et al., *"Verus: A Practical Foundation for Systems Verification"*, SOSP 2024**:
  - **Core Mechanism**: Embeds **linear ghost permissions (`tracked` mode)** directly in Rust's borrow checker! Unlike traditional separation-logic tools that simulate heap frames in an external prover, Verus uses Rust's own ownership/borrowing rules on `tracked` ghost tokens (`#[verifier::spec]` / `tracked`), which are **completely erased at compile time** (`sizeof == 0`).
- **Hawblitzel, Hance, Lattuada et al., *Verified Mimalloc (`verus-lang/verified-memory-allocator`)* (2024)**; **Zhou et al., *"VeriSMo: A Verified Security Module for Confidential VMs"*, OSDI 2024**; **Chen et al., *"Atmosphere"*, SOSP 2025**:
  - **Verified Mimalloc** is a full 7,000+ line verified port of Microsoft's high-performance `mimalloc` allocator in Rust + Verus, matching C `mimalloc` performance. Notice that `mmtk-core`'s own `FreeListAllocator` (`src/util/alloc/free_list_allocator.rs` & `native_ms`) is **explicitly modeled on Mimalloc** (`mi_bin`, sharded block lists, local/shared free lists)!
  - **How Verified Mimalloc Tames Raw Memory**:
    - Separates physical pointers (`*mut u8`, runtime `exec` mode) from **spatial memory permissions** (`tracked PointsToRaw`, erased `proof` mode).
    - `PointsToRaw(range, provenance)` supports zero-cost linear **spatial algebra**:
      - `split(mid) -> (PointsToRaw[lo..mid], PointsToRaw[mid..hi])` (carving OS `mmap` segments -> 4 MiB chunks -> 64 KiB blocks -> cells / bump regions);
      - `join(left, right) -> PointsToRaw[lo..hi]` (coalescing freed blocks/pages);
      - `into_typed::<T>() -> PointsTo<T>` (given alignment + size + bit-validity proof, safely reading/writing typed headers or array slices).
  - **Stable Rust 1.84 Realization**: Even without Verus's SMT eraser, **non-`Copy` (`!Copy`) ZST or range-carrying capability structs** (`QuarantinedRegion`, `ExclusiveChunk<'gc>`, `OwnedBlock`) implement the *exact same linear spatial splitting* in vanilla Rust 1.84! Because a ZST capability has `size_of == 0`, passing `(Address, ExclusiveChunk<'gc>)` passes a single machine register (`rdi`) in the SysV ABI — **identical machine code to raw `Address`**, while Rust's affine borrow checker statically forbids double-claiming or aliasing a chunk/region!

#### 2B. Rust Aliasing Semantics (`RustBelt`, `Stacked Borrows`, `Tree Borrows`) & Rust 1.84 `Strict Provenance`
- **Jung, Jourdan, Krebbers & Dreyer, *"RustBelt: Securing the Foundations of the Rust Programming Language"*, POPL 2018** (*JACM 2020*):
  - Formalized **semantic type soundness** in Iris: an `unsafe` library is *semantically sound* iff **no safe client can cause Undefined Behavior**, regardless of how pathological or adversarial its sequence of safe API calls is. Any `pub fn` that relies on unverified caller discipline ("only call this during GC", "only pass non-zero aligned `Address`", "only pass a spec with `num_of_bits <= 64`") violates semantic soundness (**unsafe laundering**).
- **Jung, Dang, Kang & Dreyer, *"Stacked Borrows: An Aliasing Model for Rust"*, POPL 2020**; and **Villani, Hostert, Dreyer & Jung, *"Tree Borrows"*, PLDI 2025**:
  - Formalize operational UB for Rust references (`&T` = `SharedReadOnly` unless wrapping `UnsafeCell<T>`, which grants `SharedReadWrite`; `&mut T` = `Unique` / `Reserved` -> `Active` on write).
  - **Three Lethal Consequences for GC Engines like `mmtk-core`**:
    1. **No `&'static [T]` over concurrently mutated side metadata**: If `T` does not contain `UnsafeCell` (e.g. `u8`, `u16`, `UInt256`), forming `&'static [T; N]` emits LLVM `readonly` + `dereferenceable(N * sizeof(T))` attributes. Concurrent atomic stores via raw pointers (`AtomicU8::from_ptr` / `SideMetadataSpec::store_atomic`) are **immediate UB** under both Stacked Borrows and Tree Borrows!
    2. **No `&T` + `&mut T` coexistence on non-`UnsafeCell` structs**: Holding `&BlockList` on the stack while `attempt_release` casts a side-metadata raw pointer back to `&mut BlockList` and writes `self.first` (`native_ms/block_list.rs:170, 211`) invalidates the active `SharedReadOnly` tag (under Stacked Borrows, even *creating* `&mut BlockList` invalidates `&BlockList`; under Tree Borrows, writing through `&mut` while `&BlockList` is read is UB!).
    3. **Subobject Provenance Narrowing**: `&slice[0] as *const T` carries provenance strictly over `0..size_of::<T>()`, NEVER the rest of `slice` (as `@safety-b` just flagged in `#24` on `slot.rs`!).
- **Rust 1.84 `Strict Provenance` & `Exposed Provenance` APIs** (stabilized in **Rust 1.84.0** — `mmtk-core`'s exact `rust-version`!):
  - Distinguishes **provenance-preserving** operations (`ptr.addr()`, `ptr.with_addr(a)`, `ptr.map_addr(f)`, `NonNull::with_addr`) from **exposed-provenance** integer-to-pointer casts (`std::ptr::with_exposed_provenance::<T>(addr)`, `std::ptr::with_exposed_provenance_mut::<T>(addr)`, `ptr.expose_provenance()`).
  - **Architectural Rule for `mmtk-core`**:
    - Whenever a type wraps a real Rust allocation (`Plan`, `Mutator`, `GCWorker`, `BlockList` if ever referenced, `SimpleSlot`), **keep `NonNull<T>` / `*mut T` instead of round-tripping through `usize`/`Address`** so pointer provenance is never stripped or exposed!
    - Reserve `std::ptr::with_exposed_provenance{_mut}` strictly for `Address::to_ptr` / `to_mut_ptr` over OS `mmap`-backed GC heap and side-metadata regions (eliminating `integer_to_ptr_transmutes` warnings and clarifying Miri provenance semantics).

#### 2C. Deductive Verifiers & Bounded Model Checkers (`Creusot`, `Aeneas`, `Prusti`, `Gillian-Rust`, `Kani`)
- **Denis, Jourdan & Marché, *"Creusot: A Foundry for the Deductive Verification of Rust Programs"*, ICFP 2022**; **Ho & Protzenko, *"Aeneas: Rust Verification by Functional Translation"*, ICFP 2022**; **Astrauskas et al., *"Leveraging Rust Types for Modular Specification and Verification" (`Prusti`)*, OOPSLA 2019**; **Ayoun et al., *"Gillian-Rust"*, PLDI 2024**:
  - **Prophetic Mutable Borrows**: Creusot and Aeneas show that safe `&mut T` code translates into **pure functional state transformers** $(T_{\text{now}}, T_{\text{final}})$ with **zero heap-aliasing proof obligations**, whereas raw pointers (`*mut T`) explode verification complexity quadratically. Replacing raw back-pointers (like `MS_BLOCK_LIST` in `native_ms`) with ordinary `&mut BlockList` traversal makes code simultaneously safe in Rust 1.84 and trivially verifiable!
- **Kani Rust Verifier (Amazon / AWS, open-source `cbmc`-backed bit-precise model checker)**:
  - Symbolically verifies **actual `unsafe` Rust and bit-twiddling functions** over all possible 32-bit/64-bit inputs (catching shift overflows, division/alignment errors, out-of-bounds bit ranges, and `transmute` UB) in seconds in CI with `#[cfg(kani)] #[kani::proof]` harnesses and zero changes to production code!

---

### Family 3: Generative Branding Beyond Cells — Zero-`unsafe` Bounds & Arena Indexing

- **Launchbury & Peyton Jones, *"Lazy Functional State Threads"*, PLDI 1994** (`runST` rank-2 universal quantification $\forall s.\, \text{ST}\ s\ a$); **Yanovski, Dang, Jung & Dreyer, *"GhostCell: Separating Permissions from Data in Rust"*, ICFP 2021**; **Peters, `qcell`** (`QCell`, `TCell`, `LCell`); **CAD97, `generativity`** (`make_guard!`).
- **Sverdrup (`bluss`), *The `indexing` Crate: Lifetime-Branded Bounds Proofs in Safe Rust***:
  - **What `cross_boundary_design.md` v1 Missed**: Generative lifetime branding (`PhantomData<fn(&'id ()) -> &'id ()>`, invariant in `'id`) is **not just for cells**! In `indexing`, opening a slice/table in a scoped closure ` indexing::scope(&mut slice, |container: Container<'id, _>| { ... })` fixes the slice's length for the lifetime `'id`:
    - `container.vet(i) -> Result<Index<'id>, _>` checks `i < container.len()` **once** outside a loop and returns a branded `Copy` wrapper `Index<'id>(usize, Id<'id>)`.
    - `container.vet_range(a..b) -> Result<Range<'id, NonEmpty>, _>` validates a subrange **once** and yields safe `.first()`, `.split_at()`, and iterator methods producing pre-vetted `Index<'id>` tokens that index **any parallel array of the same branded length** via `container[idx]` with **zero bounds checks AND zero `get_unchecked` `unsafe` at call sites**!
- **Const-Bounded Finite Ordinals (`Fin<const N: usize>` / `BoundedUsize<const MAX: usize>`) & Power-of-Two Masking**:
  - When a table size `N` is a compile-time constant (e.g., `SFTSpaceMap` size `32`, `SFTDenseChunkMap` size `256`, `Immix` `Block::LINES = 256`, `FreeListAllocator` `MAX_BIN + 1 = 74`), you don't even need a rank-2 closure `'id`!
  - Defining a tiny encapsulated type `#[derive(Copy, Clone)] pub struct Fin<const N: usize>(usize);` with invariant `self.0 < N` (enforced in `Fin::new(x) -> Option<Self>` or `Fin::from_masked(x)` when `N` is a power of two via `Fin(x & (N - 1))`) lets `impl<T, const N: usize> Index<Fin<N>> for [T; N]` either:
    1. Use a single centralized internal `unsafe { self.get_unchecked(i.get()) }` justified by `Fin<N>`'s closed invariant, OR
    2. When `N` is `256` (`u8 as usize`) or a power-of-two mask `self[i.get() & (N - 1)]`, let **LLVM's scalar evolution / range analysis elide the bounds check in 100% safe Rust (`#[forbid(unsafe_code)]`)**!
- **Typed & Generational Arena Handles (`slotmap` — Peters; `rustc_index::IndexVec`)**:
  - Replacing raw `*const dyn Trait` / `*mut Node` pointers in homogeneous pools (`BlockLists`, `SFTMap` space registries, GC work graphs) with compact typed indices (`u8`, `u16`, `u32` newtypes) eliminates pointer provenance issues, shrinks memory/cache footprint by 2x–8x, and turns use-after-free/dangling-pointer UB into bounded array indexing.

---

### Family 4: Bit-Validity, Layout Calculus & Safe Raw-Memory / MMIO Systems

#### 4A. Rust Project Safe Transmute, `zerocopy`, and `bytemuck` 1.14
- **Wrenn, Liebow-Feeser, Levick et al., *Rust Project Safe Transmute (`rust-lang/project-safe-transmute`)* (RFC 2930)**; **Google Fuchsia/Android `zerocopy` (`0.8`)**; **Lokathor, `bytemuck` (`1.14`, ALREADY a dependency in `mmtk-core/Cargo.toml:40-43`!)**:
  - Formalizes **bit-validity** separately from type identity via a lattice of layout/bit-pattern marker traits (verified at compile time by proc-macro derives):
    - **`Zeroable`** (`zerocopy::FromZeros`): All-zero bytes `0x00..00` represent a valid initialized value of `Self` -> enables **100% safe zero-allocation/zero-initialization** (`bytemuck::zeroed()`, `zeroed_box()`, safe `MaybeUninit` zeroing via `zeroable_maybe_uninit`, already enabled in `Cargo.toml`!).
    - **`NoUninit`** (`zerocopy::IntoBytes + Immutable`): `Self` has no padding bytes and no `UnsafeCell` -> any initialized `&Self` or `&[Self]` can be **safely viewed as `&[u8]` (`bytemuck::bytes_of`, `cast_slice`)** without reading uninitialized padding (which is instant LLVM poison UB!).
    - **`AnyBitPattern`** (`zerocopy::FromBytes`): Every bit pattern of `size_of::<Self>()` bytes is valid (no niches like `bool`, `NonZero*`, `enum` discriminants, references, or function pointers) -> any aligned `&[u8]` / word slice can be safely cast to `&Self` / `Self` (`bytemuck::pod_read_unaligned`, `from_bytes`, `cast_slice`).
    - **`Pod`** (`Plain Old Data = Copy + 'static + Zeroable + NoUninit + AnyBitPattern`): Supports bidirectional zero-cost safe casts (`bytemuck::cast::<A, B>`, `cast_slice`, `cast_slice_mut`) whenever sizes/alignments match.
    - **`CheckedBitPattern` & `Contiguous`** (`zerocopy::TryFromBytes`): For `#[repr(u8)]` / `#[repr(usize)]` enums with restricted discriminants (`Pause`, `CopySemantics`, `BlockState`, `MarkState`), auto-generates safe checked bit-pattern decoding (`checked::try_from_bits` / `Contiguous::from_integer`) with **zero `std::mem::transmute` `unsafe`** (avoiding unsound `PodInOption` bugs like the `Pause` bug `@plan` just had to fix!).

#### 4B. Safe Raw Guest/Heap Memory Views (`rust-vmm`'s `vm-memory`) & Rust-for-Linux (`Opaque<T>`, `LockedBy`, `pin-init`)
- **`rust-vmm / vm-memory` (`Firecracker` / `crosvm` / `Cloud Hypervisor` VMM ecosystem)**:
  - **Why VMMs Look Just Like GCs**: A Virtual Machine Monitor mmaps a multi-gigabyte guest physical RAM region that guest vCPUs and device threads mutate concurrently — identical to MMTk's `mmap`-backed GC heap and side metadata!
  - **Core Abstraction (`VolatileSlice<'a>` + `ByteValued` + `AtomicAccess`)**: `vm-memory` **never materializes `&[u8]` or `&mut [u8]`** over guest RAM (which would be instant Stacked/Tree Borrows UB under concurrent vCPU writes!). Instead, `VolatileSlice<'a>` wraps `(addr: *mut u8, size: usize, PhantomData<&'a ()>)` with **bounds-checked** `.subslice(offset, count)`, `.load::<T: AtomicAccess>(offset, order)`, `.store::<T: AtomicAccess>(offset, val, order)`, and `.read_obj::<T: ByteValued>(offset)`!
  - **Direct Lesson for `mmtk-core`**: Both `MetadataByteArrayRef<ENTRIES>` (`side_metadata/global.rs:1727`) and `RCArray` (`policy/immix/line.rs:265`, which STILL holds `&'static [UInt; N]`!) are ad-hoc attempts at `VolatileSlice`! Unifying them into a single `MappedMetaSlice<'a, T: Pod>` / `MappedMetaArray<'a, const N: usize, T: Pod>` backed by raw pointer + atomic/volatile ops gives **bounds-safe, aliasing-UB-free metadata bulk access** across `metadata` and `policy`!
- **Rust-for-Linux (`kernel` crate — Lossin, Ryhl, Almeida, Almeida Filho)**:
  - **`Opaque<T>`**: Wraps `MaybeUninit<UnsafeCell<T>>` (`#[repr(transparent)]`) for memory shared with C/hardware/untyped allocators that may have concurrently mutated or uninitialized bytes, preventing Rust from assuming bit-validity or `readonly`/`noalias` before explicit atomic/pointer access.
  - **`LockedBy<T, L>`**: Stores data `T` inside struct `A` (e.g. a per-block or per-space record) whose access `&T` / `&mut T` is unlocked **without a per-object lock header** by presenting `&MutexGuard<'_, L>` of a coarse lock `L` (`fn access<'a>(&'a self, guard: &'a LData) -> &'a mut T`, verified by pointer equality `ptr::eq(self.owner, guard)` — identical in spirit to `qcell::LCell` / `TCell`!).
  - **`pin-init` (Benno Lossin, Linux Kernel 6.x)**: Safe in-place initialization (`impl PinInit<T>`) directly into heap/pinned allocations (`Arc::pin_init`, `Box::pin_init`), solving the self-referential / pinned-address problem where moving a struct (`Mutator<VM>`, `Plan`) after storing raw pointers to its fields invalidates Stacked Borrows `Unique` tags and dangles pointers!

---

### Family 5: Typestate Protocols, Affine-to-Linear Idioms & Minimal-TCB OS Architectures

- **Strom & Yemini, *"Typestate: A Programming Language Concept for Enhancing Software Reliability"*, IEEE TSE 1986**; **Aldrich, Sunshine, Saini & Sparks, *"Typestate-Oriented Programming"*, OOPSLA 2009**:
  - Encodes protocol state machines as distinct zero-cost parameterizations `Region<Unmapped> -> Region<Mapped> -> Region<Zeroed>`, or phase-typed tokens (`PreparePhase`, `ClosurePhase`, `ReleasePhase`). Operations illegal in a state simply **do not exist in that state's method set**, eliminating runtime phase flags and `unreachable!()` panics.
- **Simulating True Linear ("Must-Consume") Obligations in Affine Rust**:
  - Rust's ownership is *affine* (values can be dropped silently or forgotten via `mem::forget`), not strictly *linear*. Systems Rust enforces "must complete step B" via the **Receipt / Proof-of-Completion Token** idiom:
    - Example: `fn finish_sweep(work: SweepWork) -> SweepReceipt` where the caller (`StageBarrier`) requires `SweepReceipt` (with private constructor) to advance — `mem::forget(work)` cannot forge `SweepReceipt`, so skipping cleanup is statically impossible!
- **Minimal-TCB Rust OS Architectures**:
  - **`Asterinas` Framekernel** (Peng,inas et al., *"Asterinas: A Linux ABI-Compatible, Rust-Based Framekernel OS with a Small and Sound TCB"*, **USENIX ATC 2025**): Concentrates all `unsafe` inside a minimal framework crate (`ostd`, ~14% of codebase) exporting capability types (`UntypedMem`, `UFrame`, `USegment`, `VmIo` safe read/write traits for `Pod` types, `TaskLocal` vs `CpuLocal`), leaving **100% of subsystems/drivers in `#[forbid(unsafe_code)]`**.
  - **`RedLeaf`** (Narla, Burtsev et al., **OSDI 2020**), **`Theseus`** (Boos et al., **OSDI 2020** — `MappedPages` affine region bijections), **`Tock`** (Levy et al., **SOSP 2017** — `TakeCell` / `MapCell`), and **Oxide `Hubris`** (compile-time static task/region partitioning):
  - **Architectural Takeaway for `mmtk-core`**: Treat `src/util/{address, metadata, memory, os}` + `src/vm` as MMTk's **`ostd` minimal TCB**, exporting safe capability types (`AlignedAddress`, `TypedSideSpec`, `MappedMetaArray`, `ExclusiveChunk`, `QuarantinedRegion`) so `src/policy/**` and `src/plan/**` approach **near-zero `unsafe`**!

---

### Family 6: ISMM / PL Verified & Rust Garbage Collectors

- **Blackburn & McKinley, *"Immix: A Mark-Region Garbage Collector with Space Efficiency, Fast Collection, and Mutator Performance"*, PLDI 2008**; **Zhao, Blackburn & McKinley, *"LXR: Low-Latency, High-Throughput Garbage Collection"*, PLDI 2022**:
  - Immix and LXR organize memory into a strict **2-level power-of-two spatial hierarchy**: **Blocks (`32 KiB` / `64 KiB`, aligned to `Block::BYTES`)** subdivided into **`256` Lines (`128 B` / `256 B`, indexed `0..256`)**, with per-block state (`BlockState` / `DefragState`), per-line mark bytes (`LINE_MARK_TABLE`, 256 bytes/block), and (in LXR) per-line saturating RC fields (`RC_TABLE`, `BITS_PER_LINE` bits/line).
  - Crucially, **every line index is in `0..256` (`u8` / `Fin<256>`)**, and every `Block` address is aligned to `Block::BYTES` and mapped before allocation — invariants that `mmtk-core` currently discards into raw `usize` / `Address` across `immix/{block,line}.rs` and `util/alloc/immix_allocator.rs`!
- **Mechanized GC Verification Literature**:
  - **Hawblitzel & Petrank, *"Automated Verification of Practical Garbage Collectors"*, POPL 2009** (Boogie/Z3 verification of mark-sweep & copying GCs — inspiring Verus!);
  - **Gammie, Hosking & Engelhardt, *"Relaxing Safely: Verified On-the-Fly Garbage Collection for x86-TSO"*, PLDI 2015** (concurrent mutator/collector handshake & mark-bit ordering invariants);
  - **Wang, Cao, Mansky, Sheng & Appel, *"CertiGC: An Efficiently Verified Garbage Collector"*, OOPSLA 2019** (VST/Coq separation-logic proof of generational copying GC, modeling nursery bump frames `[cursor, limit)` as split spatial byte arrays);
  - **Parkinson, Clebsch, Vytiniotis et al., *"Project Verona: Concurrency-Oriented Programming (`cown`s)"*, OOPSLA 2023** & **`snmalloc` (ISMM 2019)** (message-passing chunk/slab ownership transfer without per-object locks).
- **Safe-Rust GC Root & Handle Designs (`shifgrethor` — Lev; `gc-arena` — West; `Alloy` — Hughes & Tratt, *OOPSLA/TOPLAS*)**:
  - Show how lifetimes tie object references to safepoints (`Gc<'root, T>`), though as `cross_boundary_design.md` v1 rightly noted, MMTk's FFI VM-binding boundary (`ObjectReference(NonZeroUsize)`) fixes `ObjectReference` as `'static` ABI-wise, shifting MMTk's internal branding wins to **STW epochs (`StwToken<'stw>`), chunk ownership (`ExclusiveChunk<'gc>`), and metadata views (`MappedMetaArray<'a>`)**.

---

## 3. Paradigm Synthesis Matrix for `mmtk-core` (Rust 1.84)

| Literature Paradigm | Key Citations | Core Mechanism in Stable Rust 1.84 | `'brand` Infection? | Multi-`MMTK` Safe? | Hotpath Cost | Primary `mmtk-core` Target Slice(s) |
| :--- | :--- | :--- | :---: | :---: | :---: | :--- |
| **Const-Validated Refinement Specs** | `Flux` (PLDI '23), `TickTock` (SOSP '25), `"Parse, don't validate"` | Sealed `BitWidth` + `TypedSideSpec<R, W, V>` + `const { assert!(...) }` | **None** | **Yes** | **0 instrs** (const-folded shifts/masks!) | `metadata/{side_metadata, header_metadata}` <-> `policy/**`, `heap/chunk_map` |
| **Ghosts of Departed Proofs (GDP) + Raw Views** | Noonan ('18), `vm-memory` (`rust-vmm`), `Asterinas` (ATC '25) | `MappedMetaArray<'a, N, T>` over raw/atomic ptrs + `MappedRegion` witness (NO `&'static [T]`) | Local borrow `'a` only | **Yes** | **0 instrs** | `side_metadata/global.rs` <-> `immix/{block,line}.rs` (`RCArray`, `MetadataByteArrayRef`) |
| **Refined Address & Region Algebra** | `Flux` (PLDI '23), `RefinedRust` (PLDI '24) | `NonZeroAddress(NonZeroUsize)`, `AlignedAddress<LOG_ALIGN>`, `Region::try_from_*` | **None** | **Yes** | **0 instrs** (`NonZeroUsize` niche!) | `util/{address, linear_scan, conversions}` <-> `policy/**`, `alloc/**`, `heap/space_descriptor` |
| **Linear Spatial Capabilities (`PointsToRaw`)** | `Verus` (OOPSLA '23, SOSP '24), `Verified Mimalloc`, `Theseus` (OSDI '20) | `!Copy` ZST/range tokens: `QuarantinedRegion`, `ExclusiveChunk<'gc, S>` | Local `'gc` only | **Yes** | **0 instrs** (`sizeof == 0` ZST) | `heap/{map64, chunk_map}` <-> `util/raw_memory_freelist`, `policy/{malloc_ms, native_ms, immix}` |
| **Parent-Mutator Ownership (`&mut` vs Back-Pointers)** | `Creusot` (ICFP '22), `Aeneas` (ICFP '22), `Tree Borrows` (PLDI '25) | Mutate intrusive list via `&mut BlockList` during sweep instead of raw `BLOCK_LIST_TABLE` back-ptrs | **None** | **Yes** | **Faster + saves 8 B / 64 KiB block!** | `policy/marksweepspace/native_ms/{block, block_list, global}.rs` |
| **Bit-Validity Lattice (`bytemuck` 1.14)** | `Project Safe Transmute`, `zerocopy`, `bytemuck` | `#[derive(Pod, Zeroable, Contiguous, CheckedBitPattern)]` + `MetadataValue: Pod` | **None** | **Yes** | **0 instrs** | `immix/line.rs` (`UInt*`), `alloc/bumpallocator.rs`, `metadata_val_traits.rs`, `plan` enums |
| **Const-Bounded Ordinals & Branded Indexing** | `bluss/indexing`, `GhostCell` (ICFP '21) | `Fin<const N: usize>` (`Fin<32>`, `Fin<256>`, `Fin<74>`) + power-of-two masking / `Index<'id>` | **None** (`Fin<N>`) | **Yes** | **0 instrs** (LLVM elides bounds check; drops `Vec` deref!) | `policy/sft_map.rs`, `immix/line.rs`, `alloc/free_list_allocator.rs` (`mi_bin`) |
| **Strict Provenance & Lock-Free Static Default** | Rust 1.84 `std::ptr`, `RustBelt` (POPL '18) | `with_exposed_provenance{_mut}` for `mmap` addrs; `NonNull<T>` for Rust structs; `AtomicPtr` static default | **None** | **Yes** | **0 spills / 0 branches** | `util/address.rs`, `heap/layout/vm_layout.rs`, `vm/slot.rs` |

---

## 4. Urgent Corrections & Five Active Soundness / UB Bugs Discovered in `mmtk-core`

Before detailing the seven cross-folder Stage-2 blueprints, `@lit-arch` must address **one hot-path correction to v1 Rank 3** and incorporate fixes for **five active soundness/UB bugs in `master`** exposed by this literature review:

### 4.0 Urgent Correction to `cross_boundary_design.md` v1 Rank 3 (`VM_LAYOUT: OnceLock<VMLayout>`)
- **Problem (confirmed empirically on `@heap`'s branch in board `#22`)**: Replacing `vm_layout()` with `VM_LAYOUT.get_or_init(VMLayout::default)` injects an Acquire atomic state check, conditional branch, cold `OnceLock::initialize` call, and **3 callee-saved register spills (`push %r15/%r14/%rbx`)** directly into `SFTSpaceMap::addr_to_index` (`src/policy/sft_map.rs:259` -> `SFTSpaceMap::get_unchecked`), which executes on **every single object trace in 64-bit MMTk**!
- **Required Fix in Stage-2 Design**:
  1. In `src/util/heap/layout/vm_layout.rs`: Keep a compile-time `static DEFAULT_VM_LAYOUT: VMLayout = VMLayout::new_...()`, a `static CUSTOM_VM_LAYOUT: OnceLock<VMLayout> = OnceLock::new()`, and `static VM_LAYOUT_PTR: AtomicPtr<VMLayout> = AtomicPtr::new((&DEFAULT_VM_LAYOUT as *const VMLayout).cast_mut())`. `vm_layout() -> &'static VMLayout` performs a single branchless, stackless leaf load `unsafe { &*VM_LAYOUT_PTR.load(Ordering::Acquire) }`.
  2. **Even better across folders (`policy/sft_map.rs` + `heap/layout/map64.rs`)**: Cache `address_mask: usize` and `log_space_extent: u32` as plain `usize`/`u32` fields directly inside `struct SFTSpaceMap` (`sft_map.rs:174`) and `struct Map64` (`map64.rs`) at `new()` time so `SFTSpaceMap::addr_to_index(&self, addr)` reads `self` directly without touching any global static at all!

### 4.1 Five Newly Discovered Active Cross-Folder Soundness / UB Bugs in `master`

1. **[Bug #1 — Rudra Unvalidated-Spec UB] Safe `pub struct HeaderMetadataSpec` & `SideMetadataSpec` Trigger Misaligned Atomic & Shift-Overflow UB in `--release` (`src/util/metadata/header_metadata.rs:24-76, 133`, `src/util/metadata/side_metadata/global.rs:24-36, 125, 588`)**:
   - Both structs are `pub` in `mmtk::util::metadata` with **all `pub` fields** (`bit_offset: isize`, `num_of_bits: usize`, `log_num_of_bits: usize`, `log_bytes_in_region: usize`).
   - `HeaderMetadataSpec::load_atomic::<T>(&self, ...)` (`header_metadata.rs:107` — a **safe `pub fn`** in `@meta` or called via safe wrappers!) and `ensure_ valid` guards (`assert_spec`, `assert_value_type`) are gated behind `#[cfg(debug_assertions)]`!
   - Constructing `HeaderMetadataSpec { bit_offset: 8, num_of_bits: 64 }` in 100% safe Rust and calling `load_atomic::<u64>` on an aligned header executes `&*(byte_addr.to_ptr::<AtomicU64>())` at `header + 1` -> **instant unaligned `AtomicU64` hardware/LLVM UB in release builds**! Similarly, `num_of_bits >= 128` or `log_num_of_bits >= 7` triggers shift-overflow UB or `unreachable!()`.
2. **[Bug #2 — Stacked/Tree Borrows `'static` Shared-Slice UB] `RCArray` in `src/policy/immix/line.rs:265-278`**:
   - Look at `src/policy/immix/line.rs:265-278`:
     ```rust
     pub struct RCArray {
         table: &'static [<Uint<{ BITS_PER_LINE }> as UintType>::Type; BITS_PER_BLOCK / BITS_PER_LINE],
     }
     impl RCArray {
         pub fn of(block: Block) -> Self {
             Self { table: unsafe { &*block.rc_table_start().to_ptr() } }
         }
     }
     ```
   - `RCArray::of(block)` is a **100% safe `pub(crate)/pub fn`**:
     1. Passing an unmapped `Block` creates a dangling `&'static [UInt; N]` (`dereferenceable` UB even without indexing!).
     2. Even for a mapped `Block`, `table` is a `'static` shared reference `&'static [u8; 256]` (no `UnsafeCell`!) over `RC_TABLE` side metadata that LXR subsequently mutates via `RC_TABLE` atomics and `bzero_metadata`! Under Stacked Borrows, Tree Borrows, and LLVM `readonly` + `'static` lifetime semantics, holding/creating `&'static [u8; 256]` over memory that is ever mutated later in the program's execution is **unconditional Undefined Behavior** — the exact same bug `@meta` fixed for `MetadataByteArrayRef` in `side_metadata/global.rs`, left alive on the `policy/immix` side of the boundary!
3. **[Bug #3 — Safe-Rust `NonZeroUsize::new_unchecked(0)` Niche UB] `native_ms::Block::from_aligned_address(Address::ZERO)` & `Region::from_unaligned_address(Address::ZERO)` (`src/policy/marksweepspace/native_ms/block.rs:41-46`, `src/util/linear_scan.rs:104-106`)**:
   - `Region::from_aligned_address` and `Region::from_unaligned_address` (`linear_scan.rs:94, 104`) are **safe trait methods**.
   - `Address::ZERO` (`0usize`) satisfies `0 % 65536 == 0` (`is_aligned_to(Self::BYTES)`), and `Address::ZERO.align_down(65536)` equals `Address::ZERO`.
   - `native_ms::Block(NonZeroUsize)` implements safe `from_aligned_address(address: Address)` with only `debug_assert!(!address.is_zero())` followed by `Self(unsafe { NonZeroUsize::new_unchecked(address.as_usize()) })` (`block.rs:45`)! Calling `<native_ms::Block as Region>::from_unaligned_address(Address::ZERO)` in 100% safe Rust constructs `NonZeroUsize(0)` in release builds -> **immediate language-level validity/niche UB** (`Option<Block>::Some(0)` collides with `None`)! (Identical hazard exists in `immix::Block::from_unaligned_address(Address::ZERO)` if `immix::Block` or `Line` assumes non-zero!)
4. **[Bug #4 — Stacked/Tree Borrows `&`/`&mut` Aliasing UB + `Mutator` Move UAF + Dead Code] `MS_BLOCK_LIST` / `BLOCK_LIST_TABLE` in `native_ms` (`src/policy/marksweepspace/native_ms/{block.rs:101, 188-199, 239-250, block_list.rs:147-185, 211-236}`)**:
   - `BlockList::store_block_list(&mut self, block)` casts `self as *const BlockList as usize` (stripping pointer provenance!) and stores it in global side metadata `BLOCK_LIST_TABLE`.
   - **Hazard A (Active `&self` + `&mut *block_list` Aliasing UB on Every GC Release)**: `BlockList::release_blocks(&self, space)` (`block_list.rs:211`) and `release_and_sweep_blocks(&self, space)` (`line 225`) hold a live shared borrow `&self: &BlockList` while iterating `self.iter()` and calling `block.attempt_release(space)` (`block.rs:239`), which loads `block_list: *mut BlockList = self.load_block_list()` and calls `unsafe { &mut *block_list }.remove(self)` (`block.rs:245`, `block_list.rs:170`) — mutating `self.first` / `self.last` through `&mut BlockList` **while `&self: &BlockList` is live on the exact same call stack**!
   - **Hazard B (Dangling Pointer on `Mutator<VM>` Move)**: `FreeListAllocator` embeds `available_blocks: BlockLists` (`[BlockList; MI_BIN_FULL + 1]`) **inline inside `Mutator<VM>`**! Whenever `Box<Mutator<VM>>` is constructed on the stack and moved into `Box::new(mutator)` (or moved by a VM binding), any address previously stored in `BLOCK_LIST_TABLE` dangles!
   - **Root-Cause Discovery**: `rg load_block_list src/` proves `Block::attempt_release` (`block.rs:244`) is the **SOLE caller of `load_block_list` in the entire repository** (`SweepChunk` in `native_ms/global.rs:638-648` runs in `SweepChunk` *after* `PrepareMutator` has already unlinked unmarked blocks and never calls `attempt_release`!), AND `BlockList::lock` / `unlock` (`block_list.rs:147-160`) have **ZERO callers anywhere in `mmtk-core`**!
5. **[Bug #5 — Rudra `Send`/`Sync` Variance & `RefCell` Race Hazard] `AllocationOptionsHolder` (`src/util/alloc/allocator.rs:97-110`)**:
   - `pub struct AllocationOptionsHolder(RefCell<AllocationOptions>)` has `unsafe impl Sync for AllocationOptionsHolder {}` (`line 105`) and lives inside `pub struct AllocatorContext<VM>` (`Arc<AllocatorContext<VM>>` in `MMTK<VM>`).
   - `AllocationOptions` is literally **3 `bool`s** (`at_safepoint`, `allow_gc`, `allow_oom_call` = 3 bytes!). Wrapping 3 `bool`s in `RefCell` + `unsafe impl Sync` means two threads calling safe `AllocatorContext::set_alloc_options(&self, ...)` concurrently cause a **non-atomic data race on `RefCell`'s `Cell<BorrowFlag>` counter** -> instant UB! Packing the 3 `bool`s into a 3-bit bitmask inside a single `AtomicU8` (`Ordering::Relaxed`) deletes `RefCell`, deletes `unsafe impl Sync`, fixes the data race, and shrinks `AllocationOptionsHolder` from 16 bytes to **1 byte** with faster codegen!

---

## 5. Seven Concrete Cross-Folder Stage-2 Refactorings for `mmtk-core` (Stable Rust 1.84)

Each blueprint below maps a specific PL/verification paradigm to concrete cross-folder interfaces in `mmtk-core`, compilable on **stable Rust 1.84** with **zero hot-path overhead** and **zero unsafe laundering**.

---

### Blueprint 1: Const-Validated Refinement Specs (`Flux` / `TickTock` / `"Parse, don't validate"`)
**Cross-Folder Boundary**: `src/util/metadata/{side_metadata, header_metadata, metadata_val_traits}` <-> `src/policy/**` (`immix`, `native_ms`, `malloc_ms`, `sft_map`) & `src/util/heap/chunk_map.rs`

#### Problem
Today `SideMetadataSpec` and `HeaderMetadataSpec` have `pub` unvalidated fields (`Bug #1`), and every metadata access (`load_atomic::<u8>`, `store_atomic::<usize>`, `fetch_and_atomic::<u8>`) dynamically dispatches on `log_num_of_bits` (`0..=6`) while callers in `policy/marksweepspace/native_ms/block.rs` (`lines 108-235`), `immix/{block,line}.rs`, `malloc_ms/metadata.rs`, and `chunk_map.rs` manually cast between domain types (`Option<Block>`, `BlockState`, `ChunkState`, `VMThread`, `LineMark`) and raw integers (`usize`, `u8`) via repetitive `unsafe` blocks.

#### Design
1. **Harden `HeaderMetadataSpec` & `SideMetadataSpec` Against Release-Mode UB (`Bug #1`)**:
   - Keep `const fn` constructors (`const_new`), which **validate invariants at compile time via `assert!`** (`num_of_bits` in `{1, 2, 4, 8, 16, 32, 64}`, `bit_offset` aligned to `num_of_bits` when `>= 8`, `log_bytes_in_region < usize::BITS as usize`), and seal mutability by making fields private (with `#[inline(always)] pub const fn` getters; keep deprecated/compat `pub const fn` builders if any external binding constructs `HeaderMetadataSpec` in `const` context, validating inside `const fn new`!).
   - Because `HeaderMetadataSpec` and `SideMetadataSpec` constants (`VM::VMObjectModel::LOCAL_...`, `spec_defs::*`) are known constants at `#[inline]` call sites, LLVM constant-folds a release `assert!` on `spec` fields to **zero instructions** whenever the spec comes from a `const`, while blocking forged-spec UB from dynamic callers!
2. **Introduce Zero-Cost `TypedSideSpec<R: Region, W: BitWidth, V: MetadataCodec<W::Raw>>` Across `metadata` <-> `policy`**:
   ```rust
   pub mod sealed { pub trait Sealed {} }

   /// Compile-time witness that bit-width is in {1, 2, 4, 8, 16, 32, 64}.
   pub trait BitWidth: sealed::Sealed + Copy + 'static {
       const LOG_BITS: usize;
       const BITS: usize = 1 << Self::LOG_BITS;
       type Raw: MetadataValue;
   }
   pub struct U1; // LOG_BITS = 0, Raw = u8 (masked to 0..=1)
   pub struct U2; // LOG_BITS = 1, Raw = u8 (masked to 0..=3)
   pub struct U4; // LOG_BITS = 2, Raw = u8 (masked to 0..=15)
   pub struct U8; // LOG_BITS = 3, Raw = u8
   // ... U16, U32, U64, Uptr

   /// Total, lossless bijection between raw bit representation `Raw` and domain value `V`.
   /// Safe trait because `decode` must handle EVERY possible masked `Raw` value without UB!
   pub trait MetadataCodec<Raw>: Copy {
       fn encode(self) -> Raw;
       fn decode(raw: Raw) -> Self;
   }

   pub struct TypedSideSpec<R: Region, W: BitWidth, V: MetadataCodec<W::Raw> = <W as BitWidth>::Raw> {
       raw: SideMetadataSpec, // private: guaranteed `raw.log_num_of_bits == W::LOG_BITS`
                              // and `raw.log_bytes_in_region == R::LOG_BYTES`!
       _marker: PhantomData<fn(R) -> (W, V)>,
   }
   ```
   - Construction uses Rust 1.84 `const`: `const { assert!(SPEC.log_num_of_bits == W::LOG_BITS && SPEC.log_bytes_in_region == R::LOG_BYTES); }`.
   - Look at `native_ms/block.rs` (`NEXT_BLOCK_TABLE`, `PREV_BLOCK_TABLE`, `FREE_LIST_TABLE`, `SIZE_TABLE`, `TLS_TABLE`, `BLOCK_STATE_TABLE`):
     - Implementing safe `MetadataCodec<usize> for Option<Block>` (`0 => None`, `nz => Some(Block::from_raw_nonzero_aligned_masked(nz))`) turns `load_next_block`, `store_next_block`, `load_prev_block`, `store_prev_block`, `load_block_cell_size`, `store_block_cell_size`, `load_tls`, `store_tls`, `load_state`, `store_state` into **1-line typed calls** with **zero casts, zero shift mismatches, and zero `Option<NonZeroUsize>` niche UB**!

---

### Blueprint 2: GDP Mapped-Range Witnesses + `vm-memory`-Style Raw Views (`Noonan 2018` + `rust-vmm`)
**Cross-Folder Boundary**: `src/util/metadata/side_metadata/global.rs` <-> `src/policy/immix/{block.rs, line.rs}` & `src/plan/lxr/**`

#### Problem
- `RCArray` (`src/policy/immix/line.rs:265-278`) has active `'static` shared-reference aliasing UB (`Bug #2`).
- `MetadataByteArrayRef<const ENTRIES: usize>` (`side_metadata/global.rs:1727` <-> `immix/block.rs:165` `line_mark_table()`) duplicates raw-byte view logic only for `u8` and still requires `unsafe` construction in `immix/block.rs` because `Block` carries no mapped-metadata witness.

#### Design
1. **Replace `RCArray`'s `&'static [UInt; N]` with Unified `MappedMetaArray<'a, const N: usize, T: Pod>` (`#[forbid(unsafe_code)]` at call sites)**:
   - Like `vm-memory`'s `VolatileSlice`, `MappedMetaArray<'a, N, T>` stores `base: NonNull<T>` + `PhantomData<&'a ()>` — **never `&[T; N]`**!
   - Provides **safe, zero-bounds-check** element access indexed by `Fin<N>` (or `u8` when `N == 256`!):
     - `pub fn get(&self, idx: Fin<N>) -> T` (for `T: AtomicElement`, performs `Atomic*::from_ptr(...).load(Ordering::Relaxed)`; for wide `UInt256..UInt2048: Pod` during STW, takes `&StwToken<'_>` and reads via `ptr::read`!).
     - `pub fn set_non_atomic(&self, _stw: &StwToken<'_>, idx: Fin<N>, val: T)` (`ptr::write` proven race-free by `StwToken<'stw>`!).
2. **GDP Witness `MappedBlock<'a>` (`immix/block.rs`)**:
   - Whenever a `Block` is popped from `ImmixSpace::usable_blocks`, yielded by `ChunkMap::all_chunks` over allocated chunks, or returned by `ImmixSpace::acquire_block`, wrap it in `MappedBlock<'space>(Block, PhantomData<&'space ImmixSpace>)` (a ZST-overhead `Copy` witness whose `Block` has had `SideMetadataContext::try_map_metadata_space` succeed!).
   - `MappedBlock::line_mark_table(&self) -> MappedMetaArray<'_, { Block::LINES }, u8>` and `MappedBlock::rc_table(&self) -> MappedMetaArray<'_, { RCArray::ENTRIES }, RCUint>` become **100% safe methods** with **zero `unsafe` in `immix/{block,line}.rs`** and **zero Stacked/Tree Borrows UB**!

---

### Blueprint 3: Refined `Address` & `Region` Algebra (`Flux` / `RefinedRust`)
**Cross-Folder Boundary**: `src/util/{address.rs, linear_scan.rs, conversions.rs}` <-> `src/policy/**`, `src/util/alloc/**`, `src/util/heap/space_descriptor.rs`

#### Problem
- `Region::from_aligned_address` / `from_unaligned_address` on `Address::ZERO` causes instant `NonZeroUsize::new_unchecked(0)` UB in `native_ms::Block` (`Bug #3`).
- `NonZeroUsize -> Address -> ObjectReference` repeatedly drops `NonZero` information into `Address(usize)` and re-asserts it via `unsafe { Address::from_usize(...) }` and `unsafe { ObjectReference::from_raw_address_unchecked(...) }` across `native_ms/block.rs:49, 108, 294, 299`, `immix/{block,line}.rs`, `lxr/gc_work/mature_sweeping.rs`, `vo_bit`, `bumpallocator.rs:69`, and `space_descriptor.rs:105, 115`.

#### Design
1. **Safe Integer-to-Address Conversion Where Address Semantics Are Purely Numeric**:
   - Note that `Address::ZERO + offset` (`Add<usize> for Address`, `src/util/address.rs:482`) is **already a safe operation** in `mmtk-core` because constructing an `Address` (unlike dereferencing it!) has no validity precondition (`Address(usize)` is valid for all `usize` bit patterns!).
   - Wait — why is `Address::from_usize` `pub unsafe fn` (`address.rs:584`) when `Address::ZERO + raw` is safe `pub fn`?! Because `from_usize` is historical `org.vmmagic` ceremony! Providing an explicit internal/crate-visible safe constructor `Address::from_raw_bits(usize)` (or `NonZeroAddress(NonZeroUsize)` -> `pub const fn to_address(self) -> Address`) eliminates **dozens of vacuous `unsafe { Address::from_usize(...) }` blocks** in pure address math (`space_descriptor.rs:105, 115`, `conversions.rs`, `bumpallocator.rs:69`, `side_metadata/helpers.rs:63`, `native_ms/block.rs:49, 108, 294`) while keeping every *dereference* (`load`, `store`, `to_ptr` use) properly guarded!
2. **`NonZeroAddress` + `AlignedAddress<const LOG_ALIGN: u32>`**:
   ```rust
   #[repr(transparent)]
   #[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
   pub struct AlignedAddress<const LOG_ALIGN: u32>(NonZeroUsize);
   // Invariant: self.0 != 0 && (self.0.get() & ((1usize << LOG_ALIGN) - 1)) == 0
   ```
   - `AlignedAddress::<LOG_BYTES_IN_OBJECT>::to_object_reference(self) -> ObjectReference` is **100% safe, branchless, and 0 instructions** (both wrap `NonZeroUsize` with `OBJECT_REF_OFFSET` applied safely when non-zero)!
   - Fix `Bug #3` in `Region` (`src/util/linear_scan.rs`): make `Region::from_aligned_address` assert `!address.is_zero() && address.is_aligned_to(Self::BYTES)` in **release builds** (or add `try_from_aligned_address(Address) -> Option<Self>`), closing the `Address::ZERO` niche-UB hole in `native_ms::Block` and `immix::{Block, Line}`!

---

### Blueprint 4: Linear Spatial Capabilities & Eliminating `MS_BLOCK_LIST` Back-Pointers (`Verus` `PointsToRaw` + `Creusot` `&mut`)
**Cross-Folder Boundary**: `src/util/heap/{layout/map64.rs, chunk_map.rs}` <-> `src/util/raw_memory_freelist.rs` & `src/policy/{marksweepspace/native_ms, marksweepspace/malloc_ms, immix}`

#### Problem
1. **Eliminating `Bug #4` (`MS_BLOCK_LIST` / `BLOCK_LIST_TABLE`) in `native_ms`**:
   - Storing `*mut BlockList as usize` in `MS_BLOCK_LIST` side metadata causes `&BlockList` + `&mut BlockList` aliasing UB, `Mutator<VM>` move dangling-pointer UB, provenance loss, and wastes **8 bytes of side metadata per 64 KiB block**.
2. **Spatial Disjointness Across `Map64` -> `RawMemoryFreeList` and `ChunkMap` -> Per-Chunk Sweep**:
   - `Map64::create_parent_freelist` carves a quarantined `[start, start + list_extent)` region and hands raw `Address` arguments to `RawMemoryFreeList::new`, which creates `&'static mut [i32]` slices (`#22` item 3 & `#23` item 4).
   - `ChunkMap::generate_tasks` hands raw `Address` chunks to `SweepChunk` work packets (`malloc_ms/global.rs:584`, `native_ms/global.rs:629`, `immix/immixspace.rs`), forcing per-chunk sweepers to call `unset_page_mark`, `is_marked_unsafe`, and `bzero_metadata` via `unsafe` because nothing in the type proves per-chunk exclusivity or mapped side metadata!

#### Design
1. **Delete `MS_BLOCK_LIST` / `BLOCK_LIST_TABLE` & Dead `BlockList::{lock, unlock}` in `native_ms`**:
   - Change `BlockList::release_blocks(&mut self, space: &MarkSweepSpace<VM>)` and `release_and_sweep_blocks(&mut self, space: &MarkSweepSpace<VM>)` (already called ONLY from `FreeListAllocator::prepare(&mut self)` and `reset(&mut self)`, which hold `&mut self: &mut FreeListAllocator` -> `&mut self.available_blocks[bin]`!).
   - Unlink unmarked `Block`s **directly via `self.remove(block)` during `&mut BlockList` cursor iteration**, then call `space.release_block(block)`!
   - **Impact**:
     - **Deletes `spec_defs::MS_BLOCK_LIST`** (`global` / `local` side metadata spec) — **saving 8 bytes of side metadata per 64 KiB block** across the entire MarkSweep heap!
     - **Fixes `Bug #4` completely** (zero `&`/`&mut` aliasing UB, zero `Mutator<VM>` move dangling pointers, zero provenance-stripping `usize -> *mut BlockList` casts!).
     - **Deletes dead `BlockList::{lock, unlock}` (`spin` loop)** and eliminates **5 `unsafe` sites** across `native_ms/{block.rs, block_list.rs}` + `free_list_allocator.rs`!
2. **Linear `QuarantinedRegion` Token (`Map64` -> `RawMemoryFreeList::from_quarantined`)**:
   - Following Verus's `PointsToRaw`, define a `!Copy` capability `pub(crate) struct QuarantinedRegion { base: Address, limit: Address }` minted **at most once per space slot** by `Map64` (using an atomic `compare_exchange` on `freelist_claimed: AtomicBool` in `SpaceState`) and **consumed by value** in safe `RawMemoryFreeList::from_quarantined(region: QuarantinedRegion, ...)`.
   - It becomes **statically impossible** in safe Rust to construct two `RawMemoryFreeList` instances aliasing the same `[base, limit)` region!
3. **Separating-Conjunction Chunk Partitioning (`ChunkMap::generate_exclusive_tasks` -> `ExclusiveChunk<'gc>`)**:
   - Model the separating conjunction $\bigstar_{c \in \text{AllocatedChunks}} \text{Own}(c)$ directly in `ChunkMap::generate_tasks`:
     - Because `ChunkMap::all_chunks` iterates strictly monotonically over distinct `Chunk` addresses (`chunk_idx` $0 < 1 < \dots$) during STW (`&StwToken<'stw>`), `ChunkMap` mints a `!Copy` linear token `ExclusiveChunk<'stw> { chunk: AlignedAddress<LOG_BYTES_IN_CHUNK>, _no_copy: PhantomData<*mut ()> }` for each allocated chunk.
     - `ExclusiveChunk<'stw>` statically witnesses **both** (a) chunk-level side metadata is mapped, and (b) **no other worker holds `ExclusiveChunk` for the same 4 MiB chunk**!
     - Passing `&mut ExclusiveChunk<'stw>` to `malloc_ms` page-mark helpers (`unset_page_mark`, `is_page_marked_exclusive`, chunk `bzero_metadata`) turns non-atomic per-chunk metadata reads/writes in `malloc_ms/{global,metadata}.rs` and `native_ms/global.rs` into **100% safe operations**!

---

### Blueprint 5: `bytemuck` 1.14 Bit-Validity Lattice (`Project Safe Transmute` / `zerocopy`)
**Cross-Folder Boundary**: `Cargo.toml` (`bytemuck = "1.14"`) <-> `src/util/metadata/**`, `src/policy/immix/line.rs`, `src/util/alloc/bumpallocator.rs`, `src/plan/**`

#### Design
`bytemuck = { version = "1.14", features = ["derive", "zeroable_maybe_uninit"] }` is **already compiled into `mmtk-core`** (`Cargo.toml:40-43`), yet barely tapped!
1. **Derive `#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]` on `UInt256`, `UInt512`, `UInt1024`, `UInt2048`** (`src/policy/immix/line.rs:297-315`):
   - Every `[u64; 4]`, `[u64; 8]`, `[u64; 16]`, `[u64; 32]` wrapper (`#[repr(C)]`) is trivially `Pod + Zeroable`, enabling safe word/byte reinterpretation via `bytemuck::bytes_of` / `cast_slice` in Blueprint 2 (`MappedMetaArray`) with zero `unsafe`.
2. **Bound `MetadataValue: bytemuck::Pod`** (`src/util/metadata/metadata_val_traits.rs`):
   - `u8`, `u16`, `u32`, `u64`, `usize` all implement `bytemuck::Pod` out of the box (`bytemuck` has zero-cost impls for all primitive unsigned ints), letting `side_metadata` helpers safely zero/cast buffers without manual `MaybeUninit` `unsafe`.
3. **Replace Manual Enum `transmute`s with `#[derive(bytemuck::Contiguous)]` or `CheckedBitPattern`**:
   - For `#[repr(u8)]` state enums stored in atomics or side metadata (`BlockState` in `native_ms/block.rs`, `LineMark` / `BlockMark` in `immix`, `ChunkState` in `chunk_map.rs`, `Pause` in `global_state.rs`, `CopySemantics` in `copy_context.rs`), `bytemuck::Contiguous` (`from_integer` / `into_integer`) or total `MetadataCodec<u8>` implementations eliminate every enum `transmute` with zero UB risk.

---

### Blueprint 6: Const-Bounded Ordinals `Fin<N>` & Zero-`unsafe` `SFTMap` / `mi_bin` Indexing (`bluss/indexing`)
**Cross-Folder Boundary**: `src/util/rust_util` <-> `src/policy/sft_map.rs`, `src/util/alloc/free_list_allocator.rs`, `src/policy/immix/line.rs`

#### Problem & Live `sft_map.rs` Findings
Inspecting `src/policy/sft_map.rs` uncovered **three immediate cross-boundary wins**:
1. **Three Vestigial `unsafe impl Sync`s (`sft_map.rs:180, 344, 470`)**:
   - `SFTSpaceMap`, `SFTDenseChunkMap`, and `SFTSparseChunkMap` contain ONLY `Vec<SFTRefStorage>` (`AtomicU128`/`AtomicU64`), `Address`, and `HashMap<String, usize>` — all of which are **already auto-`Send + Sync`**!
   - Adding `: Send + Sync` to `pub trait SFTMap: Send + Sync` (`sft_map.rs:9`) deletes all **3 `unsafe impl Sync`s** in `sft_map.rs` AND removes the need for `unsafe impl Sync` workarounds on `SFT_MAP` (`#26` item 2)!
2. **Eliminate `Vec` Heap Indirection + `slice::get_unchecked` `unsafe` in `SFTSpaceMap` & `SFTDenseChunkMap` via `Fin<N>`**:
   - **In `SFTDenseChunkMap` (`sft_map.rs:338, 370-372`)**: `SFT_DENSE_CHUNK_MAP_INDEX` loads a `u8` (`0..=255`). Storing `sft: [SFTRefStorage; 256]` inline (`4 KiB`, initialized with `SFTRefStorage::default()`) means `self.sft[Self::addr_to_index(address) as usize]` is **statically proven `< 256` by `rustc`/LLVM** — **0 bounds-check instructions, 0 `Vec` pointer dereference, and 0 `get_unchecked` `unsafe`**!
   - **In `SFTSpaceMap` (`sft_map.rs:175, 204, 233, 238, 259`)**: Store `sft: [SFTRefStorage; 32]`, `address_mask: usize`, and `log_space_extent: u32` directly on `SFTSpaceMap`. Define `addr_to_index(&self, addr: Address) -> Fin<32>` using `((addr.as_usize() & self.address_mask) >> self.log_space_extent) & 31` (asserting `table_size <= 32` in `new()`). Indexing `[SFTRefStorage; 32]` with `& 31` has **0 bounds-check instructions, 0 `Vec` indirection, 0 calls to `vm_layout()`, and 0 `get_unchecked` `unsafe` across `get_unchecked`, `update`, and `clear`**!
3. **Const-Bounded `Fin<N>` Across `mi_bin` (`free_list_allocator.rs` / `native_ms`) & `LineIndex` (`immix/line.rs`)**:
   - `mi_bin(size, align) -> Fin<{ MI_BIN_FULL + 1 }>` proves at the return site that the bin index is `< 74`, allowing `available_blocks` (`[BlockList; 74]`) and size-class tables to be indexed in 100% safe Rust without repeated bounds checks.
   - `LineIndex = Fin<{ Block::LINES }>` (`Fin<256>`) unifies line offsets across `immix/{block,line}.rs` and `util/alloc/immix_allocator.rs`.
   - For `SFTSparseChunkMap` (where `max_chunks()` depends on runtime `VMLayout` on compressed 64-bit), a branded `ValidChunkIndex` (or power-of-two `Box<[SFTRefStorage; MAX_CHUNKS_32]>` on 32-bit where `MAX_CHUNKS == 1 << 10 = 1024`!) eliminates `get_unchecked` on 32-bit as well (`(addr.as_usize() >> LOG_BYTES_IN_CHUNK)` on 32-bit `usize` is **mathematically `< 1024`** (`u32 >> 22 < 1024`), so LLVM elides bounds checks on `[SFTRefStorage; 1024]` in 100% safe Rust!).

---

### Blueprint 7: Rust 1.84 Strict Provenance, `AllocationOptionsHolder` Atomic Packing & Typestate Builders
**Cross-Folder Boundary**: `src/util/address.rs`, `src/util/alloc/allocator.rs`, `src/vm/slot.rs`, `src/mmtk.rs`

1. **Replace `AllocationOptionsHolder(RefCell<AllocationOptions>)` + `unsafe impl Sync` (`Bug #5`, `src/util/alloc/allocator.rs:97-110`)**:
   - Pack the 3 booleans (`at_safepoint` bit 0, `allow_gc` bit 1, `allow_oom_call` bit 2) into `struct AllocationOptionsHolder(AtomicU8)` (`const DEFAULT: u8 = 0b110;`).
   - Deletes `RefCell`, deletes `unsafe impl Sync`, closes the `Arc<AllocatorContext>` data-race hole, and compiles to a single `movb` load/store!
2. **Adopt Rust 1.84 `Strict Provenance` / `Exposed Provenance` Cleanly**:
   - In `src/util/address.rs`: replace `self.0 as *const T` / `self.0 as *mut T` with `std::ptr::with_exposed_provenance::<T>(self.0)` and `std::ptr::with_exposed_provenance_mut::<T>(self.0)`, and `ptr as usize` in `from_ptr` / `from_mut_ptr` with `ptr.expose_provenance()`.
   - In `LeakedPlan<VM>`, `ErasedVmMutRef`, and `SimpleSlot` (`*mut Atomic<Address>` -> `Option<NonNull<Atomic<Address>>>` or preserved `*mut`): **never round-trip Rust struct pointers through `Address(usize)`**, preserving full Tree/Stacked Borrows provenance!
3. **Typestate Initialization Ordering (`MMTKBuilder -> SideMetadataInitialized -> SftInitialized -> MMTK<VM>`)**:
   - Notice in `src/mmtk.rs:189-224` and `#23` item 5 (`VMMap::create_freelist` vs `finalize_static_space_map`): boot-time initialization order constraints ("must call `initialize_side_metadata` before `initialize_sft`", "must call `finalize_static_space_map` before `create_freelist`") are currently enforced only by comments and runtime `OnceLock` panics. Typestate receipts (`SideMetadataReady` token returned by `initialize_side_metadata`, required by `Plan::initialize_sft(&self, _: SideMetadataReady)`) turn boot-order bugs into compile errors at zero cost.

---

## 6. Estimated Additional Cross-Folder Impact (Beyond `cross_boundary_design.md` v1)

| Blueprint | Affected Folders | Active UB Bugs Fixed | Est. `unsafe` Sites Eliminated | Hot-Path Impact |
| :--- | :--- | :--- | :---: | :--- |
| **0. Fix `VM_LAYOUT` Static Default + Local Mask Cache** | `heap/layout/vm_layout.rs`, `map64.rs`, `policy/sft_map.rs` | Fixes v1 Rank 3 `OnceLock` spill regression (`#22`) | ~4 | **Saves 3 register spills + branch per object trace!** |
| **1. `TypedSideSpec` + Validated `Header/SideMetadataSpec`** | `metadata/**`, `policy/{native_ms, immix, malloc_ms}`, `heap/chunk_map` | **Fixes Bug #1** (release-mode misaligned atomic / shift UB) | ~20–28 | **0 instrs** (const-folded) |
| **2. `MappedMetaArray` + `MappedBlock` GDP Witness** | `metadata/side_metadata` <-> `policy/immix/{block,line}`, `plan/lxr` | **Fixes Bug #2** (`RCArray` `'static` shared-slice UB) | ~12–16 | **0 instrs** |
| **3. `AlignedAddress` + `Address::from_raw_bits` + `Region` Fix** | `util/{address, linear_scan, conversions}` <-> `policy`, `alloc`, `heap` | **Fixes Bug #3** (`Block::from_unaligned_address(ZERO)` niche UB) | ~35–50 | **0 instrs** (`NonZeroUsize` niche) |
| **4. Delete `MS_BLOCK_LIST` + `QuarantinedRegion` + `ExclusiveChunk`** | `policy/native_ms`, `heap/{map64, chunk_map}`, `util/raw_memory_freelist`, `policy/malloc_ms` | **Fixes Bug #4** (`&`/`&mut` `BlockList` UB + `Mutator` move UAF) | ~18–25 | **Saves 8 B metadata / 64 KiB block + faster release!** |
| **5. `bytemuck` 1.14 `Pod` / `Contiguous` Derives** | `immix/line.rs`, `metadata_val_traits.rs`, `alloc`, `plan` | Prevents enum/padding transmute UB | ~10–15 | **0 instrs** |
| **6. `Fin<N>` + `SFTMap: Send + Sync` + Inline SFT Arrays** | `policy/sft_map.rs`, `alloc/free_list_allocator.rs`, `immix` | Removes 3 redundant `unsafe impl Sync`s | ~12–16 | **Faster (`[T; 32]` / `[T; 256]` drops `Vec` deref!)** |
| **7. `AllocationOptionsHolder(AtomicU8)` + Strict Provenance** | `util/alloc/allocator.rs`, `util/address.rs`, `mmtk.rs` | **Fixes Bug #5** (`RefCell` `Sync` data-race hazard) | ~4–8 | **Shrinks 16 B -> 1 B, faster load/store** |
| **Combined Additional Yield (on top of v1's ~137 sites)** | **All 6 slices** | **5 active `master` UB bugs + 1 hot-path fix** | **~115–155 additional sites** | **Net faster & smaller metadata footprint** |

Combined with `cross_boundary_design.md` v1's ~137 temporal/cell sites, Stage 2 can eliminate **~250–290 of the remaining ~465 post-Stage-1 `unsafe` sites**, confining virtually all surviving `unsafe` in `mmtk-core` to the irreducible OS/hardware/FFI TCB (`libc::mmap`, raw side-metadata atomic primitives, `ObjectModel` VM header contract, and C-ABI entrypoints).

---

## 7. Three-Tier Formal Verification Roadmap for `mmtk-core`

Because `mmtk-core`'s lowest-level metadata bit-twiddling and TCB primitives intentionally retain small, concentrated `unsafe` kernels, Stage 3+ can mechanically verify that TCB without slowing down `rustc 1.84` builds:

1. **Tier 1 — Immediate CI Bit-Precise Model Checking with `Kani` (`#[cfg(kani)]`)**:
   - Zero production dependencies; runs in minutes in GitHub Actions.
   - High-ROI proof harnesses for `mmtk-core`'s bit-manipulation TCB:
     - `side_metadata/ranges::break_bit_range` & `helpers::{address_to_contiguous_meta_address, contiguous_meta_address_to_address, meta_byte_lshift, meta_byte_mask}` (proving exact round-trip invertibility, alignment, and absence of shift/arithmetic overflow for all valid `SideMetadataSpec` parameters);
     - `HeaderMetadataSpec` bit-masking & sign/zero extension;
     - `SpaceDescriptor` bit-packing (`create_descriptor` <-> `get_start` / `get_extent` across 32-bit and 64-bit layouts — directly preventing bugs like `#23` item 2!);
     - `free_list_allocator::mi_bin` (`size -> bin -> bin_size >= size` and `bin <= MI_BIN_FULL` for all `size`).
2. **Tier 2 — SMT Liquid Type Refinements with `Flux` (`#[cfg_attr(flux, flux::sig(...))]`)**:
   - Following **TickTock (SOSP 2025)**, attach optional `flux` refinement signatures gated behind `#[cfg_attr(flux, ...)]` (100% invisible to stable `rustc 1.84`!).
   - Verify `Address[a]` alignment (`a % ALIGN == 0`), `BumpPointer` cursor-limit invariants (`cursor <= limit`, `cursor + aligned_size <= limit`), `Fin<N>[v: v < N]`, and `ImmixAllocator` line/block cursor bounds without a single runtime `assert!` on fast paths.
3. **Tier 3 — Mechanized Separation-Logic Proofs for Core TCB Capsules (`Verus` / `RefinedRust`)**:
   - Verify semantic soundness (`RustBelt` / `RefinedRust` PLDI '24) of the standalone ~50-line concurrency/spatial primitives (`StwCell`, `WorkerLocal`, `QuarantinedRegion` + `RawMemoryFreeList`, and `ExclusiveChunk`), mirroring **Verified Mimalloc (`verus-lang/verified-memory-allocator`)**.

---

## References

1. **Frampton, Blackburn, Cheng, Garner, Grove, Moss & Salishev**, *"Demystifying Magic: High-Level Low-Level Programming"*, **ACM SIGPLAN/SIGOPS VEE 2009**.
2. **Lin, Blackburn, Hosking & Norrish**, *"Rust as a Language for High Performance GC Implementation"*, **ACM SIGPLAN ISMM 2016**.
3. **Blackburn & McKinley**, *"Immix: A Mark-Region Garbage Collector with Space Efficiency, Fast Collection, and Mutator Performance"*, **ACM SIGPLAN PLDI 2008**.
4. **Zhao, Blackburn & McKinley**, *"LXR: Low-Latency, High-Throughput Garbage Collection"*, **ACM SIGPLAN PLDI 2022**.
5. **Lehmann, Geller, Vazou & Jhala**, *"Flux: Liquid Types for Rust"*, **ACM SIGPLAN PLDI 2023** (*PACMPL* 7, PLDI).
6. **Rindisbacher, Johnson, Lehmann, Potyondy, Pannuto, Savage, Stefan & Jhala**, *"TickTock: Verified Isolation in a Production Embedded OS"*, **ACM SOSP 2025**.
7. **Gäher, Sammler, Jung, Krebbers & Dreyer**, *"RefinedRust: A Type System for High-Assurance Verification of Rust Programs"*, **ACM SIGPLAN PLDI 2024**.
8. **Sammler, Lepigre, Krebbers, Memarian, Dreyer & Garg**, *"RefinedC: Automating the Foundational Verification of C Code with Refined Ownership Types"*, **ACM SIGPLAN PLDI 2021**.
9. **Pulte, Dhokia, Kay et al.**, *"CN: Verifying Systems C Code with Separation-Logic Refinement Types"*, **ACM SIGPLAN POPL 2023**.
10. **Noonan**, *"Ghosts of Departed Proofs (Functional Pearl)"*, **ACM SIGPLAN Haskell Symposium 2018**.
11. **Kiselyov & Shan**, *"Lightweight Static Capabilities"*, **ACM SIGPLAN PLPV 2006**.
12. **Launchbury & Peyton Jones**, *"Lazy Functional State Threads"*, **ACM SIGPLAN PLDI 1994**.
13. **Lattuada, Hance, Cho, Brun, Subasinghe, Zhou, Howell, Parno & Hawblitzel**, *"Verus: Verifying Rust Programs using Linear Ghost Types"*, **ACM SIGPLAN OOPSLA 2023**; and *"Verus: A Practical Foundation for Systems Verification"*, **ACM SOSP 2024**.
14. **Hawblitzel, Hance, Lattuada et al.**, *"Verified Mimalloc (`verus-lang/verified-memory-allocator`)"*, 2024; **Hawblitzel & Petrank**, *"Automated Verification of Practical Garbage Collectors"*, **ACM SIGPLAN POPL 2009**.
15. **Zhou et al.**, *"VeriSMo: A Verified Security Module for Confidential VMs"*, **USENIX OSDI 2024**; **Chen et al.**, *"Atmosphere"*, **ACM SOSP 2025**.
16. **Jung, Jourdan, Krebbers & Dreyer**, *"RustBelt: Securing the Foundations of the Rust Programming Language"*, **ACM SIGPLAN POPL 2018** (*JACM* 2020).
17. **Jung, Dang, Kang & Dreyer**, *"Stacked Borrows: An Aliasing Model for Rust"*, **ACM SIGPLAN POPL 2020**.
18. **Villani, Hostert, Dreyer & Jung**, *"Tree Borrows"*, **ACM SIGPLAN PLDI 2025**.
19. **Denis, Jourdan & Marché**, *"Creusot: A Foundry for the Deductive Verification of Rust Programs"*, **ACM SIGPLAN ICFP 2022**; **Ho & Protzenko**, *"Aeneas: Rust Verification by Functional Translation"*, **ACM SIGPLAN ICFP 2022**; **Astrauskas et al.**, *"Prusti"*, **OOPSLA 2019**; **Ayoun et al.**, *"Gillian-Rust"*, **PLDI 2024**.
20. **Yanovski, Dang, Jung & Dreyer**, *"GhostCell: Separating Permissions from Data in Rust"*, **ACM SIGPLAN ICFP 2021**.
21. **Sverdrup (`bluss`)**, *`indexing`: Sound Unchecked Indexing Using Lifetimes in Rust*; **Peters**, *`qcell`* & *`slotmap`*.
22. **Wrenn, Liebow-Feeser, Levick et al.**, *Rust Project Safe Transmute (RFC 2930)*; **Google `zerocopy`**; **Lokathor, `bytemuck`**.
23. **`rust-vmm` Contributors**, *`vm-memory`: Safe Abstractions for Guest VM Physical Memory (`VolatileSlice`, `ByteValued`, `AtomicAccess`)*.
24. **Lossin, Ryhl, Almeida & Almeida Filho**, *Rust-for-Linux Kernel Abstractions (`pin-init`, `Opaque<T>`, `LockedBy`)*.
25. **Peng et al.**, *"Asterinas: A Linux ABI-Compatible, Rust-Based Framekernel OS with a Small and Sound TCB"*, **USENIX ATC 2025**; **Boos et al.**, *"Theseus"*, **OSDI 2020**; **Narla et al.**, *"RedLeaf"*, **OSDI 2020**; **Levy et al.**, *"Tock"*, **SOSP 2017**.
26. **Strom & Yemini**, *"Typestate"*, **IEEE TSE 1986**; **Aldrich et al.**, *"Typestate-Oriented Programming"*, **OOPSLA 2009**.
27. **Astrauskas, Matheja, Poli, Müller & Summers**, *"How Do Programmers Use Unsafe Rust?"*, **ACM SIGPLAN OOPSLA 2020**.
28. **Bae, Kim, Askar, Lim & Kim**, *"Rudra: Finding Memory Safety Bugs in Rust at the Ecosystem Scale"*, **ACM SOSP 2021**.
29. **Gammie, Hosking & Engelhardt**, *"Relaxing Safely: Verified On-the-Fly Garbage Collection for x86-TSO"*, **ACM SIGPLAN PLDI 2015**; **Wang et al.**, *"CertiGC"*, **OOPSLA 2019**; **Parkinson et al.**, *"Project Verona"*, **OOPSLA 2023** & *`snmalloc`*, **ISMM 2019**.
