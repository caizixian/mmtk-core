# Step Analysis (auto-saved)

## Target
- Files: `src/util/metadata/global.rs`, `src/util/metadata/side_metadata/global.rs`, `src/util/rust_util/atomic_box.rs`
- Strategy: Re-evaluate "NOT to Revisit" files and look for Phase 2 abstractions.

## Findings
- `src/util/metadata/global.rs`: `load` and `store` are unsafe because they call `VM::VMObjectModel::load_metadata` which is unsafe in the trait. This is part of the VM binding API and cannot be easily made safe without breaking compatibility or using capability tokens everywhere.
- `src/util/metadata/side_metadata/global.rs`: `set_raw_byte_atomic` is unsafe by design as an optimization that may corrupt adjacent bits. `bcopy_metadata_contiguous` uses `std::ptr::copy` which requires unsafe as it operates on raw `Address` types.
- `src/util/rust_util/atomic_box.rs`: `OnceOptionBox` is confirmed irreducible to maintain minimal space overhead in `Vec<OnceOptionBox>`, as noted by previous agents.

## Attempted Changes
- None. All files with significant unsafe counts listed in the prompt are either FFI calls, primitive pointer operations, or implementations of abstractions (like `MetadataCursor` in `helpers.rs`).

## Blockers / Insights for Next Step
- The codebase has already been highly optimized for unsafe reduction, dropping from 331 to 94. Remaining unsafe are mostly irreducible core operations or FFI.
- Future steps should focus on finding files with 1 or 2 unsafe blocks that might not be listed in the prompt or marked as irreducible, if any exist and can be found without violating the grep rule.
