# Step Analysis (auto-saved)

## Target
- File: `src/vm/slot.rs`, `src/util/rust_util/mod.rs`, `src/util/alloc/allocators.rs`, `src/policy/sft_map.rs`
- Strategy: Re-verify remaining unsafe locations in these files to confirm irreducibility and proper encapsulation.

## Findings
- `src/vm/slot.rs`:
  - Lines 181, 189: `SimpleSlot` loads and stores require dereferencing raw pointers derived from `Address`. This is inherently unsafe as `Address` does not guarantee validity. Trait `Slot` defines these as safe, making `SimpleSlot` a safe abstraction assuming valid construction. Irreducible.
  - Line 287: `MemorySlice::copy` uses `std::ptr::copy`. Irreducible without changing `MemorySlice` to use safe slices, which is hard due to aliasing and lack of lifetimes in GC heap.
- `src/util/rust_util/mod.rs`:
  - `ProofCell` uses unsafe in `get_ref` (unsafe fn) and `get_mut_with_proof` (unsafe block). The latter is a safe abstraction using `ExclusivePlanAccessProof`. The former is used to avoid threading proof tokens on frequent paths. Irreducible without complex refactoring to thread proof tokens.
- `src/util/alloc/allocators.rs`:
  - Uses `MaybeUninit` for layout compatibility with VM bindings. `assume_init_ref` and `assume_init_mut` are protected by runtime asserts on initialization flags. Irreducible without losing layout compatibility.
- `src/policy/sft_map.rs`:
  - `SFTRefStorage::load` uses unsafe to dereference raw pointers to atomize fat pointers (`dyn SFT`). This is a specific optimization for lock-free access on hot paths. Irreducible.

## Attempted Changes
- None. Verified that all analyzed locations are irreducible or properly encapsulated with existing safe abstractions and have appropriate SAFETY comments.

## Blockers / Insights for Next Step
- All files with unsafe in the harness list appear to be marked as irreducible in `UNSAFE_MEMORY.md` or verified as such in this step.
- The repository has reached a state where further reduction requires breaking FFI layout compatibility or adding runtime overhead to hot paths, which is not advisable.
- I will leave the Work Queue empty in `UNSAFE_MEMORY.md` as no actionable items remain.
