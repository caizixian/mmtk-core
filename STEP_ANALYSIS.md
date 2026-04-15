# Step Analysis (auto-saved)

## Target
- File: `src/policy/marksweepspace/native_ms/global.rs`
- Strategy: Fix compile errors in `native_ms/global.rs` by updating `generate_sweep_tasks` return type.

## Findings
- `generate_sweep_tasks` returns `Vec<Box<dyn GCWork>>` but should return `Vec<Box<dyn GCWork + Send>>` because it calls `chunk_map.generate_tasks`.
- This should fix Error 3 and Error 4.

## Attempted Changes
- Modifying `src/policy/marksweepspace/native_ms/global.rs` to update `generate_sweep_tasks` return type.

## Blockers / Insights for Next Step
- Need to check if other files also need similar updates (e.g., `compressorspace.rs`).
