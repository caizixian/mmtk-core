# Unsafe Analysis Knowledge Base

## Progress
- Starting count: 331 | Current: <pending> | Δ: <pending>
- Phase: 2

## Codebase Invariants (PROTECTED — do not prune)
- `SFT_MAP` is a global static `InitializeOnce` container. Accessing it mutably during plan initialization requires `unsafe` to bypass borrow checker.
- `GCWork` trait requires `'static` references for work packets, leading to lifetime extension unsafe blocks in space `prepare`/`release` methods.
- `Address::from_usize` is a safe `const fn` now. Unsafe blocks wrapping only this call are redundant.

## Work Queue (NEXT STEP: pick the first actionable item)
1. 🔴 HIGH: `src/util/metadata/log_bit.rs` — Check for unsafe blocks and see if they can be removed or abstracted. — expected Δ: unknown.

## Patterns Discovered
- Redundant `unsafe` blocks wrapping safe functions like `Address::from_usize`.
- Using `MockObject` in tests to encapsulate unsafe `load`/`store` calls on `HeaderMetadataSpec`.

## Files NOT to Revisit (all remaining unsafe is irreducible)
- `src/policy/marksweepspace/native_ms/global.rs` — Irreducible lifetime extension for `GCWork` packets [Phase 2 confirmed].
- `src/util/metadata/side_metadata/sanity.rs` — Fixed redundant unsafe block, remaining are irreducible or valid assertions [Phase 2 confirmed].
- `src/util/metadata/side_metadata/global.rs` — Production unsafe in `load`/`store` is irreducible due to concurrent access invariants [Phase 1 analysis].
- `src/util/metadata/side_metadata/helpers.rs` — Implementation of `MetadataCursor` abstraction, irreducible without moving unsafe to call sites [Phase 1 analysis].
- `src/util/metadata/side_metadata/side_metadata_tests.rs` — Irreducible raw pointer manipulations for test verification [Phase 1 analysis].
- `src/util/metadata/side_metadata/ranges.rs` — Irreducible raw pointer manipulations for bit range operations [Phase 1 analysis].
- `src/policy/sft_map.rs` — Irreducible transmute for atomic fat pointers in `SFTRefStorage` [Phase 2 confirmed].
- `src/vm/slot.rs` — Irreducible raw pointer dereferences in `SimpleSlot` and `Address` impls [Phase 2 confirmed].
- `src/util/metadata/metadata_val_traits.rs` — Trait methods require unsafe for raw pointer dereferencing in atomic operations [Phase 1 analysis].
- `src/util/metadata/pin_bit.rs` — Fixed unsafe block by using load_atomic. Remaining code is safe [Phase 1 analysis].

## Abstraction Proposals (for Phase 2)
### MetadataCursor for side_metadata
- Target files: `src/util/metadata/side_metadata/global.rs`, `src/util/metadata/side_metadata/sanity.rs`
- Expected Δ: N/A (already implemented, but centralizes unsafe)
- Design sketch: `struct MetadataCursor(Address);` provides safe methods for load/store.
- Status: done
