# Unsafe Analysis Memory

## Summary
- Total unsafe at start: 722
- Current unsafe count: 705
- Categories: FFI=?, RawHeapAccess=?, UncheckedCall=17 (Eliminated)

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

### src/util/address.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 395 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` in tests |
| 413 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` in tests |
| 431 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` in tests |
| 441 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` in tests |
| 455 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` in tests |

## Refactoring Ideas
- Replace `MaybeUninit::uninit().assume_init()` with `std::array::from_fn(|_| MaybeUninit::uninit())` in `src/util/alloc/allocators.rs`. (DONE)
