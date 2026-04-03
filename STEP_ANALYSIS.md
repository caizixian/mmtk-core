# Step Analysis (auto-saved)

## Target
- File: N/A (Holistic Audit)
- Strategy: Audit remaining unsafe code and document irreducibility rationale.

## Findings
- `src/mmtk.rs`: `StwProtected` uses `UnsafeCell` to avoid locking overhead. Safety relies on `StwProof` token and external invariant that mutation only happens when the world is stopped. Irreducible for performance.
- `src/policy/sft_map.rs`: `SFTRefStorage` uses `transmute` to store fat pointers (`&dyn SFT`) in double-word atomics. Irreducible due to lack of safe fat pointer atomics in Rust.
- `src/util/rust_util/mod.rs`: `InitializeOnce` avoids overhead of `OnceLock` on hot paths. Irreducible for performance.

## Attempted Changes
- None. Documented findings.

## Blockers / Insights for Next Step
- Confirmed that remaining unsafe blocks are likely irreducible with current architecture.
