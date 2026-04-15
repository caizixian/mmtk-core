# Step Analysis (auto-saved)

## Target
- File: `src/util/rust_util/mod.rs`
- Strategy: Analyze `InitializeOnce` and `ProofCell` for potential unsafe reduction.

## Findings
- Line 76: `unsafe { &mut *self.v.get() }.write(init_fn());` — Irreducible. Dereferencing raw pointer from `UnsafeCell` requires unsafe. `InitializeOnce` is used for performance (zero-cost reads on hot paths) instead of `OnceLock`.
- Line 87: `unsafe { (*self.v.get()).assume_init_ref() }` — Irreducible. See above.
- Line 117: `pub unsafe fn get_ref(&self) -> &T {` — Irreducible. Marked unsafe because caller must ensure no concurrent mutable accesses.
- Line 119: `unsafe { &*self.value.get() }` — Irreducible. Dereferencing raw pointer from `UnsafeCell`.
- Line 126: `unsafe { &mut *self.value.get() }` — Irreducible. Dereferencing raw pointer from `UnsafeCell` inside a safe method with proof token.
- Line 130: `unsafe impl<T: Sync> Sync for ProofCell<T> {}` — Irreducible. Required to allow sharing `ProofCell` across threads, justified by proof token requirement for mutation.

## Attempted Changes
- None. Confirmed all unsafe in this file is irreducible or properly encapsulated.

## Blockers / Insights for Next Step
- Confirmed `InitializeOnce` and `ProofCell` are irreducible to maintain zero-cost reads as documented in `UNSAFE_MEMORY.md`.
- No further work needed on this file.
