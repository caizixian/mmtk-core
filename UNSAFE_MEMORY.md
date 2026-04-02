# Unsafe Analysis Memory

## Summary
- Total unsafe at start: 722
- Current unsafe count: 674
- Categories: FFI=?, RawHeapAccess=?, UncheckedCall=47 (Eliminated), MutableStatic=2 (Eliminated)

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
| 465 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` in tests |

### src/util/heap/layout/vm_layout.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 135 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 136 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 149 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 151 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 170 | MutableStatic | ELIMINATED | Replaced `static mut VM_LAYOUT` with `OnceLock` |
| 202 | MutableStatic | ELIMINATED | Replaced `static mut VM_LAYOUT` with `OnceLock` |

### src/util/metadata/side_metadata/side_metadata_tests.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 42 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO` |
| 46 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO` |
| 51 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 55 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 60 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 64 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 72 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO` |
| 76 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO` |
| 81 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 85 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 90 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 94 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 102 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO` |
| 106 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO` |
| 111 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 115 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 120 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 124 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 160 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO` |
| 164 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 168 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 175 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO` |
| 179 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 183 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 187 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |

## Refactoring Ideas
- Replace `MaybeUninit::uninit().assume_init()` with `std::array::from_fn(|_| MaybeUninit::uninit())` in `src/util/alloc/allocators.rs`. (DONE)
