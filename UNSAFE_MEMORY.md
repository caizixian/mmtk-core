# Unsafe Analysis Memory

## Summary
- Total unsafe at start: 722
- Current unsafe count: 716
- Categories: FFI=?, RawHeapAccess=?, UncheckedCall=6 (Eliminated)

## Analyzed Files
### src/util/alloc/allocators.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 108 | UncheckedCall | ELIMINATED | Replaced with `std::array::from_fn` |
| 109 | UncheckedCall | ELIMINATED | Replaced with `std::array::from_fn` |
| 110 | UncheckedCall | ELIMINATED | Replaced with `std::array::from_fn` |
| 111 | UncheckedCall | ELIMINATED | Replaced with `std::array::from_fn` |
| 112 | UncheckedCall | ELIMINATED | Replaced with `std::array::from_fn` |
| 113 | UncheckedCall | ELIMINATED | Replaced with `std::array::from_fn` |

## Refactoring Ideas
- Replace `MaybeUninit::uninit().assume_init()` with `std::array::from_fn(|_| MaybeUninit::uninit())` in `src/util/alloc/allocators.rs`. (DONE)
