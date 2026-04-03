# Step Analysis (auto-saved)

## Target
- File: `src/util/address.rs`, `src/util/metadata/side_metadata/global.rs`, `src/vm/slot.rs`, `src/mmtk.rs`
- Strategy: Holistic review of remaining unsafe blocks under Strategy Escalation.

## Findings
- `src/util/address.rs`: The 8 unsafe locations are core primitive operations (`load`, `store`, `atomic_load`, etc.) on the `Address` type, which is a transparent wrapper around `usize`. These operations are inherently unsafe as they dereference raw memory. They cannot be made safe without a system to guarantee address validity, which is not feasible for a low-level GC toolkit.
- `src/util/metadata/side_metadata/global.rs`: The unsafe blocks in `MetadataSlot` (`get_ref`, `get_mut_ref`) are the primitive operations that enable the safe abstraction. Centralizing them here avoids spreading unsafe blocks to all call sites (e.g., `load_val`, `store_val`). Making these helpers `unsafe fn` would increase the unsafe count at call sites.
- `src/vm/slot.rs`: `SimpleSlot::as_atomic` is a primitive operation that enables the `SimpleSlot` abstraction. `MemorySlice::copy` uses `std::ptr::copy`, which is irreducible without marking the trait method `unsafe fn`, which would increase the count at all call sites.
- `src/mmtk.rs`: `StwProtected` provides a safe interface for access during stop-the-world phases. The unsafe blocks inside it are the minimal set required to implement this abstraction using `UnsafeCell`.

## Attempted Changes
- None. Verified that all remaining unsafe blocks are either irreducible primitives, well-encapsulated in safe abstractions, or FFI calls.

## Blockers / Insights for Next Step
- The codebase is confirmed to be in a steady state for Phase 3 (Irreducible Documentation). All 57 remaining unsafe blocks are justified. Documenting this conclusion is the only remaining task.
