# Step Analysis (auto-saved)

## Target
- File: `src/util/alloc/allocators.rs` and `src/plan/concurrent/mod.rs`
- Strategy: Re-verify irreducibility of remaining unsafe blocks.

## Findings
- `src/util/alloc/allocators.rs`: The unsafe blocks at lines 53 and 93 are calls to `assume_init_ref` and `assume_init_mut` on `MaybeUninit` elements in arrays. These are necessary because the arrays are partially initialized and the struct layout must be stable and without overhead (so `Option` cannot be used if it adds a tag). The unsafe operations are encapsulated in safe getter functions.
- `src/plan/concurrent/mod.rs`: The unsafe impls at lines 27 and 29 for `bytemuck::ZeroableInOption` and `bytemuck::PodInOption` on `Pause` are justified by the fact that `Pause` is a `repr(u8)` enum with values 1, 2, and 3, making 0 a valid niche for `Option<Pause>`. Since `bytemuck` does not provide derive macros for these specific traits in this context, manual unsafe impls are required.

## Attempted Changes
- Analyzed the feasibility of replacing `MaybeUninit` with `Option` in `Allocators` and concluded it would likely break layout compatibility or add overhead.
- Analyzed the safety of `bytemuck` impls in `concurrent/mod.rs` and confirmed they are correct but manual unsafe is required.

## Blockers / Insights for Next Step
- All remaining unsafe locations listed in the prompt are in files already marked as "NOT to Revisit" or are similarly irreducible. No reducible unsafe blocks were identified in this step.
