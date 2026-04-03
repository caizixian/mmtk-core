# Step Analysis (auto-saved)

## Target
- File: `src/scheduler/affinity.rs`
- Strategy: Use `core_affinity` crate to remove unsafe FFI calls for setting thread affinity.

## Findings
- Line 43-48 (original): `bind_current_thread_to_core` used unsafe FFI calls.
- Line 61-68 (original): `bind_current_thread_to_cpuset` used unsafe FFI calls.

## Attempted Changes
- Added `core_affinity = "0.8"` to `Cargo.toml`.
- Refactor `bind_current_thread_to_core` to use `core_affinity::set_for_current` which is safe and cross-platform.
- Kept `bind_current_thread_to_cpuset` as is because `core_affinity` does not support setting affinity to multiple cores.

## Blockers / Insights for Next Step
- Need to run `cargo check` to verify the refactoring.
- If it works, this will reduce the unsafe count by 1 block (or more if counting removals of unimplemented functions or blocks).
