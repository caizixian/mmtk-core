# Step Analysis (auto-saved)

## Target
- File: Holistic review of remaining unsafe files
- Strategy: Re-evaluate "irreducible" files for potential new abstractions or confirm irreducibility.

## Findings
- `src/util/metadata/side_metadata/global.rs`:
  - Lines 22, 27, 32: `MetadataSlot` helpers (`as_atomic_u8`, `get_ref`, `get_mut_ref`) contain unsafe blocks to dereference `Address`. This is a deliberate choice to centralize unsafe and keep call sites safe.
  - Line 550: `std::ptr::copy` in `bcopy_metadata_contiguous`. Irreducible without safe slice wrappers, which would still require unsafe to create from raw addresses.
- `src/policy/sft_map.rs`:
  - Line 111: `get_sft_wrapper` extends the lifetime of `&dyn SFT` to `'static`. This might be avoidable if we can change the trait and method signatures to require `&'static (dyn SFT + Sync)`, but this may have wide-reaching effects on how spaces are stored.
- `src/vm/slot.rs`:
  - Line 177: `SimpleSlot::as_atomic` centralizes raw pointer cast. Similar to `MetadataSlot`, making it unsafe would increase count at call sites.

## Attempted Changes
- None in this step. Focused on holistic review as requested by the strategy escalation.

## Blockers / Insights for Next Step
- Confirmed that most remaining unsafe blocks are either core primitives (Address), FFI wrappers (memory.rs), or centralized helpers designed to keep the overall count low.
- Next step should investigate if `get_sft_wrapper` in `sft_map.rs` can be refactored to take a `'static` reference, or if this is blocked by non-`'static` spaces.
