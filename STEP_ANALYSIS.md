# Step Analysis (auto-saved)

## Target
- File: src/util/rust_util/mod.rs
- Strategy: Investigate InitializeOnce unsafe blocks

## Findings
- Line 74: `unsafe` block to write to `UnsafeCell`. This is protected by `Once::call_once`, so it is safe in terms of races, but compiler requires unsafe.
- Line 83: `unsafe` block to call `assume_init_ref()` on `UnsafeCell` content. This has no runtime check in release builds for performance reasons, as documented in the file.
- Line 93: `unsafe fn get_mut` returns `&mut T` from `&self`. This is inherently unsafe and requires the caller to ensure no races.
- Line 96: `unsafe` block to call `assume_init_mut()`.
- Line 107: `unsafe impl Sync` is unconditional.

Conclusion: `InitializeOnce` is designed to avoid initialization checks on every read for performance (critical for `SFT_MAP` in GC fast path). Replacing it with `OnceLock` would introduce atomic checks on every read. Thus, these unsafe blocks are irreducible if we want to maintain this performance property.

## Attempted Changes
- None. Determined to be irreducible for performance reasons.

## Blockers / Insights for Next Step
- `InitializeOnce` in `src/util/rust_util/mod.rs` should be added to "Files NOT to Revisit".
- Pick a new target from the list of unsafe locations.
