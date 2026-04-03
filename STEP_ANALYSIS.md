# Step Analysis (auto-saved)

## Target
- File: src/util/metadata/side_metadata/global.rs, src/policy/sft_map.rs, src/vm/slot.rs, src/util/address.rs
- Strategy: Holistic review of remaining unsafe locations to confirm irreducibility under Strategy Escalation.

## Findings
- `src/util/metadata/side_metadata/global.rs`: `MetadataSlot::get_ref` and `get_mut_ref` contain unsafe blocks to cast raw addresses to references. They are helper methods that keep the unsafe count low at call sites. Making them `unsafe fn` would push the unsafe to safe trait implementations, increasing the count. Irreducible.
- `src/policy/sft_map.rs`: `get_sft_wrapper` extends lifetime to `'static` because spaces live forever, but the compiler cannot infer it. `SFTRefStorage::load` dereferences a raw pointer from `AtomicPtr`. Both are necessary for lock-free access and zero-overhead. Irreducible.
- `src/vm/slot.rs`: `SimpleSlot::as_atomic` casts raw address to `&Atomic<Address>`. `MemorySlice::copy` uses `std::ptr::copy`. Both are low-level memory operations. Irreducible.
- `src/util/address.rs`: `from_raw_address_unchecked` uses `NonZeroUsize::new_unchecked`. It must remain `unsafe fn` to prevent UB in release builds if called with zero. Irreducible.

## Attempted Changes
- None. Analyzed the locations and confirmed they are irreducible or locally optimal as documented in `UNSAFE_MEMORY.md`.

## Blockers / Insights for Next Step
- The codebase is genuinely in Phase 3. All remaining unsafe blocks are FFI, raw heap access, or low-level optimizations that cannot be safely abstracted further without performance loss or moving unsafe to callers.
