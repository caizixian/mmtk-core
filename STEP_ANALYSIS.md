# Step Analysis (auto-saved)

## Target
- File: `src/util/metadata/side_metadata/global.rs` and `src/policy/sft_map.rs`
- Strategy: Investigating capability tokens (`StwProof`) and lifetime constraints to remove unsafe blocks.

## Findings
- `src/util/metadata/side_metadata/global.rs`: `SideMetadataSpec::load` and `store` are non-atomic and require unsafe. We considered requiring `&StwProof` to make them safe. However, this would require all call sites to provide a proof. If they don't have one, they would need to create it unsafely, pushing the unsafe to call sites.
- `src/policy/sft_map.rs`: Line 111 uses unsafe to cast `&dyn SFT` to `&'static dyn SFT`. We considered changing the trait to require `&'static dyn SFT`. However, callers (like spaces) are owned by the plan and cannot easily prove `'static` to the compiler without leaking or unsafe casts, which would just move the unsafe to call sites.

## Attempted Changes
- Analyzed the feasibility of adding `&StwProof` to `load`/`store` and changing `SFTMap` trait. Determined both would likely increase total unsafe count or complexity at call sites.

## Blockers / Insights for Next Step
- The codebase has reached a point where remaining unsafe blocks are heavily tied to architectural decisions (ownership models, performance requirements for non-atomics). Local reductions are exhausted.
