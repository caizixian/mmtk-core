# Unsafe Analysis Memory

## Summary
- Total unsafe at start: 722
- Current unsafe count: 599
- Categories: FFI=?, RawHeapAccess=?, UncheckedCall=104 (Eliminated), MutableStatic=2 (Eliminated), UnsafeTraitImpl=3 (Eliminated), RawPointerDeref=8 (Eliminated), UnionAccess=11 (Eliminated)

## Analyzed Files
### src/util/alloc/bumpallocator.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 69 | UncheckedCall | ELIMINATED | Replaced Address::zero() with Address::ZERO |
| 228 | UncheckedCall | ELIMINATED | Replaced Address::from_usize with Address::ZERO.add |

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

### src/policy/sft_map.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 180 | UnsafeTraitImpl | ELIMINATED | Removed redundant `unsafe impl Sync for SFTSpaceMap` |
| 203 | UncheckedCall | ELIMINATED | Replaced slice `get_unchecked` with standard indexing |
| 217 | RawPointerDeref | ELIMINATED | Replaced raw pointer with reference in SFTSpaceMap::update |
| 232 | UncheckedCall | ELIMINATED | Replaced slice `get_unchecked` with standard indexing |
| 237 | UncheckedCall | ELIMINATED | Replaced slice `get_unchecked` with standard indexing |
| 344 | UnsafeTraitImpl | ELIMINATED | Removed redundant `unsafe impl Sync for SFTDenseChunkMap` |
| 370 | UncheckedCall | ELIMINATED | Replaced slice `get_unchecked` with standard indexing |
| 375 | RawPointerDeref | ELIMINATED | Replaced raw pointer with reference in SFTDenseChunkMap::notify_space_creation |
| 405 | RawPointerDeref | ELIMINATED | Replaced raw pointer with reference in SFTDenseChunkMap::update |
| 470 | UnsafeTraitImpl | ELIMINATED | Removed redundant `unsafe impl Sync for SFTSparseChunkMap` |
| 487 | UncheckedCall | ELIMINATED | Replaced slice `get_unchecked` with standard indexing |
| 499 | RawPointerDeref | ELIMINATED | Replaced raw pointer with reference in SFTSparseChunkMap::update |
| 504 | RawPointerDeref | ELIMINATED | Replaced raw pointer with reference in SFTSparseChunkMap::update |
| 604 | UncheckedCall | ELIMINATED | Replaced slice `get_unchecked` with standard indexing |

### src/util/metadata/header_metadata.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 687 | UncheckedCall | ELIMINATED | Replaced manual allocation with `Vec` |
| 689 | UncheckedCall | ELIMINATED | Replaced manual allocation with `Vec` |
| 691 | RawPointerDeref | ELIMINATED | Replaced manual allocation with `Vec` |
| 692 | RawPointerDeref | ELIMINATED | Replaced manual allocation with `Vec` |
| 693 | RawPointerDeref | ELIMINATED | Replaced manual allocation with `Vec` |
| 699 | UncheckedCall | ELIMINATED | Replaced manual allocation with `Vec` |

### src/policy/marksweepspace/native_ms/block.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 44 | UncheckedCall | ELIMINATED | Replaced `NonZeroUsize::new_unchecked` with `NonZeroUsize::new().unwrap()` |
| 48 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 104 | UncheckedCall | KEPT | Separated `Address::from_usize` (safe) from `load` (unsafe) |
| 113 | UncheckedCall | KEPT | Separated `Address::from_usize` (safe) from `load` (unsafe) |
| 124 | UncheckedCall | KEPT | Separated `Address::from_usize` (safe) from `load_atomic` (unsafe) |
| 214 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 290 | UncheckedCall | ELIMINATED | Replaced `Address::zero()` with `Address::ZERO` |

### src/util/metadata/side_metadata/helpers.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 64 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 436-443 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` in tests |
| 499-506 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` in tests |
| 548-557 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` in tests |

### src/util/metadata/side_metadata/global.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 54 | UnionAccess | ELIMINATED | Refactored `SideMetadataOffset` to `enum` |
| 63 | UnionAccess | ELIMINATED | Refactored `SideMetadataOffset` to `enum` |
| 67 | UnionAccess | ELIMINATED | Refactored `SideMetadataOffset` to `enum` |
| 77 | UnionAccess | ELIMINATED | Refactored `SideMetadataOffset` to `enum` |
| 98 | UnionAccess | ELIMINATED | Refactored `SideMetadataOffset` to `enum` |
| 108 | UnionAccess | ELIMINATED | Refactored `SideMetadataOffset` to `enum` |
| 1272 | UnionAccess | ELIMINATED | Refactored `SideMetadataOffset` to `enum` |
| 1313-1324 | UnionAccess | ELIMINATED | Refactored `SideMetadataOffset` to `enum` |
| 1602 | UnionAccess | ELIMINATED | Refactored `SideMetadataOffset` to `enum` |

### src/util/linear_scan.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 193 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 194 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 214 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 237 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 250 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |

### src/util/conversions.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 42 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 103 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 105 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 113 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 115 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 118 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |

### src/util/heap/layout/map64.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 36 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |

### src/util/heap/space_descriptor.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 101 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 111 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |

## Refactoring Ideas
- Replace `MaybeUninit::uninit().assume_init()` with `std::array::from_fn(|_| MaybeUninit::uninit())` in `src/util/alloc/allocators.rs`. (DONE)
