# Unsafe Analysis Memory

## Summary
- Total unsafe at start: 722
- Current unsafe count: 710
- Categories: FFI=?, RawHeapAccess=?, UncheckedCall=12 (Eliminated)

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

### src/util/copy/mod.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 183 | UncheckedCall | ELIMINATED | Replaced with `std::array::from_fn` |
| 184 | UncheckedCall | ELIMINATED | Replaced with `std::array::from_fn` |
| 185 | UncheckedCall | ELIMINATED | Replaced with `std::array::from_fn` |
| 224 | UncheckedCall | ELIMINATED | Replaced with `std::array::from_fn` |
| 225 | UncheckedCall | ELIMINATED | Replaced with `std::array::from_fn` |
| 226 | UncheckedCall | ELIMINATED | Replaced with `std::array::from_fn` |

## Refactoring Ideas
- Replace `MaybeUninit::uninit().assume_init()` with `std::array::from_fn(|_| MaybeUninit::uninit())` in `src/util/alloc/allocators.rs`. (DONE)
