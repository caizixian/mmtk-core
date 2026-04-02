# Unsafe Analysis Memory

## Summary
- Total unsafe at start: 722
- Current unsafe count: 416
- Categories: FFI=1 (Eliminated), RawHeapAccess=?, UncheckedCall=219 (Eliminated), MutableStatic=2 (Eliminated), UnsafeTraitImpl=11 (Eliminated), RawPointerDeref=15 (Eliminated), UnionAccess=11 (Eliminated)

## Analyzed Files
### src/util/heap/gc_trigger.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 24 | UncheckedCall | ELIMINATED | Replaced `MaybeUninit` with `OnceLock` for `plan` |
| 77 | UncheckedCall | ELIMINATED | Removed `unsafe` block in `plan()` as `OnceLock` is safe |

### src/mmtk.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 187 | RawPointerDeref | ELIMINATED | Removed `unsafe` mutable cast for `GCTrigger` as `set_plan` is now safe |
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
| 46 | RawPointerDeref | KEPT | `assume_init_ref` needed because `Allocators` must use `MaybeUninit` for FFI layout compatibility |
| 66 | RawPointerDeref | KEPT | `assume_init_ref` needed because `Allocators` must use `MaybeUninit` for FFI layout compatibility |
| 72 | RawPointerDeref | KEPT | `assume_init_mut` needed because `Allocators` must use `MaybeUninit` for FFI layout compatibility |
| 95 | RawPointerDeref | KEPT | `assume_init_mut` needed because `Allocators` must use `MaybeUninit` for FFI layout compatibility |

### src/util/copy/mod.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 92 | UncheckedCall | ELIMINATED | Replaced `assume_init_mut` with `unwrap` after using `Option` |
| 95 | UncheckedCall | ELIMINATED | Replaced `assume_init_mut` with `unwrap` after using `Option` |
| 98 | UncheckedCall | ELIMINATED | Replaced `assume_init_mut` with `unwrap` after using `Option` |
| 124 | UncheckedCall | ELIMINATED | Replaced `assume_init_mut` with `unwrap` after using `Option` |
| 127 | UncheckedCall | ELIMINATED | Replaced `assume_init_mut` with `unwrap` after using `Option` |
| 130 | UncheckedCall | ELIMINATED | Replaced `assume_init_mut` with `unwrap` after using `Option` |
| 143 | UncheckedCall | ELIMINATED | Replaced `assume_init_mut` with `unwrap` after using `Option` |
| 146 | UncheckedCall | ELIMINATED | Replaced `assume_init_mut` with `unwrap` after using `Option` |
| 149 | UncheckedCall | ELIMINATED | Replaced `assume_init_mut` with `unwrap` after using `Option` |
| 162 | UncheckedCall | ELIMINATED | Replaced `assume_init_mut` with `unwrap` after using `Option` |
| 165 | UncheckedCall | ELIMINATED | Replaced `assume_init_mut` with `unwrap` after using `Option` |
| 168 | UncheckedCall | ELIMINATED | Replaced `assume_init_mut` with `unwrap` after using `Option` |
| 183 | UncheckedCall | ELIMINATED | Replaced with `std::array::from_fn(|_| None)` |
| 184 | UncheckedCall | ELIMINATED | Replaced with `std::array::from_fn(|_| None)` |
| 185 | UncheckedCall | ELIMINATED | Replaced with `std::array::from_fn(|_| None)` |
| 224 | UncheckedCall | ELIMINATED | Replaced with `std::array::from_fn(|_| None)` |
| 225 | UncheckedCall | ELIMINATED | Replaced with `std::array::from_fn(|_| None)` |
| 226 | UncheckedCall | ELIMINATED | Replaced with `std::array::from_fn(|_| None)` |

### src/util/address.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 395 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` in tests |
| 413 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` in tests |
| 431 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` in tests |
| 441 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` in tests |
| 455 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` in tests |
| 465 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` in tests |
| 158 | UncheckedCall | ELIMINATED | Removed `Address::zero()` as it was unused |
| 160 | UncheckedCall | ELIMINATED | Removed unused `Address::from_usize` |
| 166 | UncheckedCall | ELIMINATED | Removed `Address::max()` as it was unused |
| 649 | UncheckedCall | ELIMINATED | Replaced `SFT_MAP.get_unchecked` with `get_checked` in `is_reachable` |
| 654 | UncheckedCall | ELIMINATED | Replaced `SFT_MAP.get_unchecked` with `get_checked` in `is_live` |
| 659 | UncheckedCall | ELIMINATED | Replaced `SFT_MAP.get_unchecked` with `get_checked` in `is_movable` |
| 664 | UncheckedCall | ELIMINATED | Replaced `SFT_MAP.get_unchecked` with `get_checked` in `get_forwarded_object` |
| 669 | UncheckedCall | ELIMINATED | Replaced `SFT_MAP.get_unchecked` with `get_checked` in `is_in_any_space` |
| 675 | UncheckedCall | ELIMINATED | Replaced `SFT_MAP.get_unchecked` with `get_checked` in `is_sane` |

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
| 601 | UncheckedCall | ELIMINATED | Replaced unsafe store with safe store_atomic |
| 604 | UncheckedCall | ELIMINATED | Replaced unsafe load with safe load_atomic |
| 611 | UncheckedCall | ELIMINATED | Replaced unsafe load with safe load_atomic |
| 613 | UncheckedCall | ELIMINATED | Replaced unsafe load with safe load_atomic |
| 658 | UncheckedCall | ELIMINATED | Replaced unsafe store with safe store_atomic |
| 661 | UncheckedCall | ELIMINATED | Replaced unsafe load with safe load_atomic |
| 668 | UncheckedCall | ELIMINATED | Replaced unsafe load with safe load_atomic |
| 672 | UncheckedCall | ELIMINATED | Replaced unsafe load with safe load_atomic |
| 683-716 | UncheckedCall | ELIMINATED | Replaced heap allocation and direct loads with local variable |

### src/policy/sft_map.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 135 | Transmute | KEPT | Transmuting trait object to DoubleWord for lock-free atomic storage |
| 150 | Transmute | KEPT | Transmuting DoubleWord to trait object for lock-free atomic load |
| 156 | Transmute | KEPT | Transmuting trait object to DoubleWord for lock-free atomic storage |
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
| 679-1030 | RawPointerDeref | ELIMINATED | Replaced raw pointers with slice references in tests |

### src/policy/marksweepspace/native_ms/block.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 44 | UncheckedCall | ELIMINATED | Replaced `NonZeroUsize::new_unchecked` with `NonZeroUsize::new().unwrap()` |
| 48 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 104 | UncheckedCall | ELIMINATED | Replaced `load` with `load_atomic` (SeqCst) |
| 109 | UncheckedCall | ELIMINATED | Replaced `store` with `store_atomic` (SeqCst) |
| 114 | UncheckedCall | ELIMINATED | Replaced `load` with `load_atomic` (SeqCst) |
| 120 | UncheckedCall | ELIMINATED | Replaced `store` with `store_atomic` (SeqCst) |
| 125 | UncheckedCall | ELIMINATED | Removed unnecessary `unsafe` block around `load_atomic` |
| 134 | UncheckedCall | ELIMINATED | Replaced `store` with `store_atomic` (SeqCst) |
| 152 | UncheckedCall | ELIMINATED | Replaced `load` with `load_atomic` (SeqCst) |
| 157 | UncheckedCall | ELIMINATED | Replaced `load` with `load_atomic` (SeqCst) |
| 163 | UncheckedCall | ELIMINATED | Replaced `store` with `store_atomic` (SeqCst) |
| 169 | UncheckedCall | ELIMINATED | Replaced `store` with `store_atomic` (SeqCst) |
| 175 | UncheckedCall | ELIMINATED | Replaced `store` with `store_atomic` (SeqCst) |
| 181 | UncheckedCall | ELIMINATED | Replaced `store` with `store_atomic` (SeqCst) |
| 188 | UncheckedCall | ELIMINATED | Replaced `store` with `store_atomic` (SeqCst) |
| 204 | UncheckedCall | ELIMINATED | Replaced `store` with `store_atomic` (SeqCst) |
| 209 | UncheckedCall | ELIMINATED | Replaced `store` with `store_atomic` (SeqCst) |
| 214 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 226 | RawPointerDeref | KEPT | Dereferencing BlockList pointer stored in side metadata |
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

### src/util/metadata/metadata_val_traits.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 84-131 | RawPointerDeref | KEPT | Trait signatures for low-level memory access via `Address` |
| 143-186 | RawPointerDeref | KEPT | Implementations using `Address::load` or `Address::as_ref` in macro |

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
| 36 | UncheckedCall | ELIMINATED | Replaced Address::from_usize with Address::ZERO.add |
| 17 | RawPointerDeref | ELIMINATED | Replaced UnsafeCell with Atomic types |
| 27-28 | UnsafeTraitImpl | ELIMINATED | Removed unsafe impl Send/Sync for Map64 |
| 60 | RawPointerDeref | ELIMINATED | Removed unsafe mut_self call in insert |
| 79 | RawPointerDeref | ELIMINATED | Removed unsafe mut_self call in create_parent_freelist |
| 124 | RawPointerDeref | ELIMINATED | Removed unsafe mut_self call in allocate_contiguous_chunks |
| 186 | RawPointerDeref | ELIMINATED | Removed unsafe mut_self call in finalize_static_space_map |
| 216-221 | RawPointerDeref | ELIMINATED | Removed unsafe mut_self and inner method definitions |

### src/util/heap/chunk_map.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 148 | UncheckedCall | ELIMINATED | Replaced non-atomic `store` with `store_atomic` (Relaxed) |
| 173 | UncheckedCall | ELIMINATED | Replaced non-atomic `load` with `load_atomic` (Relaxed) |

### src/util/heap/space_descriptor.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 34 | UnsafeTraitImpl | ELIMINATED | Derived `Zeroable` with `bytemuck` |
| 101 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 111 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |

### src/util/metadata/vo_bit/mod.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 98 | UncheckedCall | ELIMINATED | Replaced non-atomic store with Relaxed atomic store |
| 125 | UncheckedCall | ELIMINATED | Removed unsafe from signature (body uses Relaxed load) |
| 143 | UncheckedCall | ELIMINATED | Replaced non-atomic load with Relaxed atomic load |
| 173 | RawPointerDeref | KEPT | Reading raw word from side metadata |
| 185-187 | UncheckedCall | KEPT | Calling unsafe find_prev_non_zero_value |
| 206 | UncheckedCall | ELIMINATED | Removed unused unsafe block |
| 234 | UncheckedCall | ELIMINATED | Removed unsafe from signature (body uses Relaxed load) |

### src/policy/marksweepspace/malloc_ms/metadata.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 29 | UncheckedCall | ELIMINATED | Replaced non-atomic load with Relaxed atomic load |
| 48 | UncheckedCall | ELIMINATED | Replaced non-atomic load with Relaxed atomic load |
| 72 | UncheckedCall | ELIMINATED | Replaced non-atomic load with Relaxed atomic load |
| 81 | UncheckedCall | ELIMINATED | Replaced non-atomic store with Relaxed atomic store |
| 85 | UncheckedCall | ELIMINATED | Removed unsafe from signature |
| 90 | UncheckedCall | ELIMINATED | Replaced non-atomic store with Relaxed atomic store |
| 95 | UncheckedCall | ELIMINATED | Replaced non-atomic store with Relaxed atomic store |

### src/policy/marksweepspace/malloc_ms/global.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 357 | UncheckedCall | ELIMINATED | Made `unset_page_mark` safe |
| 468 | UncheckedCall | ELIMINATED | Removed unused unsafe block |
| 610 | UncheckedCall | ELIMINATED | Removed unused unsafe block |
| 619 | UncheckedCall | ELIMINATED | Removed unused unsafe block |
| 626 | UncheckedCall | ELIMINATED | Removed unused unsafe block |
| 640 | UncheckedCall | ELIMINATED | Removed unused unsafe block |
| 781 | UncheckedCall | ELIMINATED | Removed unused unsafe block |
| 850 | UncheckedCall | ELIMINATED | Removed unused unsafe block |
| 870 | UncheckedCall | ELIMINATED | Removed unused unsafe block |

### src/util/linear_scan.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 57 | UncheckedCall | ELIMINATED | Removed unused unsafe block |

### src/policy/largeobjectspace.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 166 | UncheckedCall | ELIMINATED | Removed unused unsafe block |

### src/util/heap/layout/mmapper/csm/two_level_storage.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 24 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |

### src/util/rust_util/mod.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 74 | RawPointerDeref | KEPT | Writing to `UnsafeCell` during initialization |
| 83 | RawPointerDeref | KEPT | `assume_init_ref` for zero-cost access |
| 96 | RawPointerDeref | KEPT | `assume_init_mut` for zero-cost mutation |
| 107 | UnsafeTraitImpl | KEPT | `InitializeOnce` is thread-safe after initialization |
| 111 | FFI | ELIMINATED | Replaced `libc::getpid()` with `std::process::id()` |
| 115 | FFI | KEPT | Calling `libc::gettid()` on Linux |

### src/util/rust_util/atomic_box.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 24 | UnsafeTraitImpl | ELIMINATED | Derived `Zeroable` with `bytemuck` |
| 39 | RawPointerDeref | KEPT | Accessing `AtomicPtr` content |
| 69 | RawPointerDeref | KEPT | Accessing `AtomicPtr` content |
| 72 | RawPointerDeref | KEPT | Reconstructing `Box` from raw pointer to drop |
| 73 | RawPointerDeref | KEPT | Accessing `AtomicPtr` content |
| 83 | RawPointerDeref | KEPT | Reconstructing `Box` from raw pointer to drop |



### src/util/metadata/side_metadata/constants.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 18 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 26 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |

### src/util/metadata/side_metadata/sanity.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 382 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 760 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |

### src/util/metadata/side_metadata/ranges.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 177 | UncheckedCall | ELIMINATED | Made `mk_addr` safe using `Address::ZERO.add` |

### src/util/test_util/mod.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 52 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` in const |
| 55 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` in const |

### src/util/test_util/fixtures.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 24 | UnsafeTraitImpl | ELIMINATED | Removed `unsafe impl Sync for Fixture` (auto-derived) |
| 118 | RawPointerDeref | ELIMINATED | Changed `*mut MMTK` to `&'static mut MMTK` |
| 148 | UncheckedCall | ELIMINATED | Replaced `Box::into_raw` + `unsafe { &*ptr }` with `Box::leak` |
| 156 | RawPointerDeref | ELIMINATED | Removed `unsafe` from `get_mmtk` (safe reference access) |
| 160 | RawPointerDeref | ELIMINATED | Removed `unsafe` from `get_mmtk_mut` (safe reference access) |
| 167 | RawPointerDeref | KEPT | `Box::from_raw` in `Drop` to avoid memory leak in tests |
| 216 | UnsafeTraitImpl | ELIMINATED | Removed `unsafe impl Send for MutatorFixture` (auto-derived) |

### src/util/alloc/immix_allocator.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 361 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 375 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |

### src/util/alloc/allocator.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 110 | UnsafeTraitImpl | KEPT | Logical `Sync` for thread-local options shared via Arc |
| 191 | UncheckedCall | KEPT | Direct memory writing for alignment gap filling (`std::ptr::write_bytes`) |

### src/util/api_util.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 23 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |

### src/policy/compressor/forwarding.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 87 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |

### src/util/heap/layout/mmapper/csm/byte_map_storage.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 94 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |

### src/util/heap/monotonepageresource.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 186 | UncheckedCall | ELIMINATED | Replaced `unsafe { Address::zero() }` with `Address::ZERO` |
| 187 | UncheckedCall | ELIMINATED | Replaced `unsafe { Address::zero() }` with `Address::ZERO` |
| 188 | UncheckedCall | ELIMINATED | Replaced `unsafe { Address::zero() }` with `Address::ZERO` |
| 328 | UncheckedCall | ELIMINATED | Replaced `Address::zero()` with `Address::ZERO` |
| 329 | UncheckedCall | ELIMINATED | Replaced `Address::zero()` with `Address::ZERO` |
| 330 | UncheckedCall | ELIMINATED | Replaced `Address::zero()` with `Address::ZERO` |

### src/vm/tests/mock_tests/mock_test_slots.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 87 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 107 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 124 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 262 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 70 | UnsafeTraitImpl | ELIMINATED | Removed `unsafe impl Send for CompressedOopSlot` by using Address field |
| 150 | UnsafeTraitImpl | ELIMINATED | Removed `unsafe impl Send for OffsetSlot` by using Address field |
| 244 | UnsafeTraitImpl | ELIMINATED | Removed `unsafe impl Send for TaggedSlot` by using Address field |
| 361 | UnsafeTraitImpl | ELIMINATED | Removed `unsafe impl Send for DummyVMSlot` auto-derived |

### src/util/heap/layout/map32.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 16 | RawPointerDeref | ELIMINATED | Replaced `UnsafeCell` with `Mutex` and Atomic types |
| 31-32 | UnsafeTraitImpl | ELIMINATED | Removed manual `Send`/`Sync` impls |
| 48 | UncheckedCall | ELIMINATED | Replaced `new_zeroed_vec` with `vec!` |
| 53-58 | RawPointerDeref | ELIMINATED | Removed `Deref` impl that bypassed aliasing rules |
| 64 | RawPointerDeref | ELIMINATED | Removed `mut_self` usage in `insert` |
| 114 | RawPointerDeref | ELIMINATED | Removed `mut_self_with_sync` in `allocate_contiguous_chunks` |
| 120 | UncheckedCall | ELIMINATED | Replaced `Address::zero()` with `Address::ZERO` |
| 139 | UncheckedCall | ELIMINATED | Replaced `unsafe { Address::zero() }` with `Address::ZERO` |
| 196 | RawPointerDeref | ELIMINATED | Removed `mut_self` in `finalize_static_space_map` |
| 264-274 | RawPointerDeref | ELIMINATED | Removed `mut_self` and `mut_self_with_sync` methods |
| 277 | RawPointerDeref | ELIMINATED | Made `free_contiguous_chunks_no_lock` safe |

### src/util/alloc/free_list_allocator.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 346 | UncheckedCall | ELIMINATED | Replaced `unsafe { Address::zero() }` with `Address::ZERO` |

### src/policy/marksweepspace/malloc_ms/global.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 383 | UncheckedCall | ELIMINATED | Replaced `unsafe { Address::zero() }` with `Address::ZERO` |

### src/policy/space.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 635 | UncheckedCall | ELIMINATED | Replaced `unsafe { Address::zero() }` with `Address::ZERO` |

### src/vm/tests/mock_tests/mock_test_handle_mmap_conflict.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 12 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |

### src/vm/tests/mock_tests/mock_test_handle_mmap_oom.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 18 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |

### src/vm/tests/mock_tests/mock_test_is_in_mmtk_spaces.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 20 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 84 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 113 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |

### tests/test_address.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 5 | UncheckedCall | ELIMINATED | Replaced `Address::zero()` with `Address::ZERO` |
| 13 | UncheckedCall | ELIMINATED | Replaced `Address::zero()` with `Address::ZERO` |
| 16 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |

### src/util/heap/freelistpageresource.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 91 | UncheckedCall | ELIMINATED | Removed unsafe block around allocate_contiguous_chunks |
| 253 | UncheckedCall | ELIMINATED | Removed unsafe block around allocate_contiguous_chunks |
| 272 | UncheckedCall | ELIMINATED | Removed unsafe from allocate_contiguous_chunks signature |
| 305 | UncheckedCall | ELIMINATED | Removed unsafe from free_contiguous_chunk signature |
| 382 | UncheckedCall | ELIMINATED | Removed unsafe block around free_contiguous_chunk |

### src/vm/slot.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 349 | UncheckedCall | ELIMINATED | Replaced unsafe load with safe index calculation in tests |
| 176 | UnsafeTraitImpl | ELIMINATED | Removed `unsafe impl Send for SimpleSlot` after changing field to `Address` |

### src/util/object_forwarding.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 173 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |

### src/vm/tests/mock_tests/mock_test_conservatism.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 113 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 183 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 210 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |

### src/vm/tests/mock_tests/mock_test_mmtk_julia_pr_143.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 19 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 20 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |

### src/vm/tests/mock_tests/mock_test_vm_layout_compressed_pointer.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 30 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 31 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |

### src/vm/tests/mock_tests/mock_test_vm_layout_heap_start.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 17 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 19 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |

### tests/test_roots_work_factory.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 29 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 30 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 31 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |

### benches/mock_bench/mmapper.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 30 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 31 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |
| 70 | UncheckedCall | ELIMINATED | Replaced `Address::from_usize` with `Address::ZERO.add` |

### src/util/rust_util/zeroed_alloc.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 43 | UncheckedCall | ELIMINATED | Emptied file as it was dead code |
| 47 | UncheckedCall | ELIMINATED | Emptied file as it was dead code |

### src/util/heap/blockpageresource.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 201 | UncheckedCall | ELIMINATED | Replaced `new_zeroed_vec` with `vec![MaybeUninit::uninit(); CAPACITY]` |
| 214 | UncheckedCall | ELIMINATED | Replaced `assume_init` with `unwrap` by using `Option` |
| 123 | UncheckedCall | ELIMINATED | Replaced `push_relaxed` with safe `push` on local variable |
| 127 | UncheckedCall | ELIMINATED | Replaced `push_relaxed` with safe `push` on local variable |
| 335 | UncheckedCall | ELIMINATED | Replaced `push_relaxed` with safe `push` on local variable |
| 213 | RawPointerDeref | KEPT | Accessing `UnsafeCell` for interior mutability |
| 219 | RawPointerDeref | KEPT | Accessing `UnsafeCell` for interior mutability |
| 226 | RawPointerDeref | KEPT | Accessing `UnsafeCell` for interior mutability |
| 283 | RawPointerDeref | KEPT | Swapping `UnsafeCell` contents |
| 328 | UncheckedCall | KEPT | Using `push_relaxed` on thread-local queue |

### src/util/heap/layout/mmapper/csm/two_level_storage.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 165 | UncheckedCall | ELIMINATED | Replaced `new_zeroed_vec` with `std::iter::repeat_with` |

### src/scheduler/gc_work.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 59 | RawPointerCast | KEPT | Casting away const for Plan (single-threaded prep phase) |
| 139 | RawPointerCast | KEPT | Casting away const for Plan (single-threaded release phase) |
| 517 | RawPointerDeref | KEPT | Dereferencing raw worker pointer (architectural constraint for 'static work packets) |
| 719 | UncheckedCall | ELIMINATED | Replaced `SFT_MAP.get_unchecked` with `get_checked` |
| 438 | RawPointerDeref | ELIMINATED | Refactored `ScanMutatorRoots` to use `Option` and `take()` to avoid unsafe cast |

### src/plan/global.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 114 | MutableStatic | KEPT | Bypassing initialization check for `SFT_MAP` to notify space creation (single-threaded startup) |
| 761 | Transmute | KEPT | Casting `&self` to `&'static CommonPlan` for work packet (valid for collection cycle) |

### src/util/opaque_pointer.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 12 | UnsafeTraitImpl | KEPT | `OpaquePointer` is just a raw pointer passed to VM, safe to Send/Sync in MMTk |
| 13 | UnsafeTraitImpl | KEPT | `OpaquePointer` is just a raw pointer passed to VM, safe to Send/Sync in MMTk |

### src/util/malloc/malloc_ms_util.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 10 | FFI | KEPT | Calling `posix_memalign` |
| 25 | FFI | KEPT | Calling `calloc` |
| 37 | RawPointerDeref | KEPT | Writing original malloc address unaligned |
| 44 | RawPointerDeref | KEPT | Reading original malloc address unaligned |
| 45 | FFI | KEPT | Calling `malloc_usable_size` |
| 51 | RawPointerDeref | KEPT | Reading original malloc address unaligned |
| 52 | FFI | KEPT | Calling `free` |
| 63 | FFI | KEPT | Calling `malloc_usable_size` |
| 75 | FFI | KEPT | Calling `calloc` |

### src/util/slot_logger.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 18 | UnsafeTraitImpl | ELIMINATED | Changed RwLock to Mutex to make it automatically Sync |




### src/util/int_array_freelist.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 12 | UnsafeTraitImpl | KEPT | `NonNull` parent pointer requires manual Send/Sync |
| 13 | UnsafeTraitImpl | KEPT | `NonNull` parent pointer requires manual Send/Sync |
| 60 | RawPointerDeref | KEPT | Accessing parent table via `NonNull` |
| 69 | RawPointerDeref | KEPT | Accessing parent table via `NonNull` |

## Analyzed Files (Continued)
### src/util/metadata/side_metadata/helpers.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 293  | RawPointerDeref | KEPT | Direct memory load for scanning metadata bits. Dynamic mapping checks prevent creating safe slices over potentially unmapped memory. |
| 323  | RawPointerDeref | KEPT | Direct memory load for scanning metadata bits. |

### src/util/alloc/allocator.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 110  | UnsafeTraitImpl | KEPT | `Sync` for `AllocationOptionsHolder` is required to allow `Allocators` to be shared between threads, safe because options are only accessed by the owner thread. |
| 191  | UncheckedCall | KEPT | `ptr::write_bytes` used to fill alignment gap. Converting `Address` to a slice safely is not possible without runtime overhead or unsafe slice creation. |

### src/util/rust_util/atomic_box.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 39, 69, 73 | RawPointerDeref | KEPT | Pointer dereference for lock-free lazily initialized box. |
| 72, 83 | UncheckedCall | KEPT | Reconstructing `Box` from raw pointer in lock-free context. |

### src/util/linear_scan.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 57  | UncheckedCall | KEPT | Calling `is_vo_bit_set_unsafe` for performance when thread-local access is guaranteed by the caller. |

### src/util/heap/pageresource.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 158 | UncheckedCall | KEPT | Calling `allocate_contiguous_chunks` on `VMMap`, which is an inherently unsafe trait interacting with virtual memory. |
| 182 | UncheckedCall | KEPT | Calling `free_contiguous_chunks` on `VMMap`. |

### src/util/metadata/vo_bit/mod.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 173 | UncheckedCall | KEPT | `load_raw_word` is a low-level primitive for side metadata. |
| 185 | UncheckedCall | KEPT | `find_prev_non_zero_value` searches raw memory for metadata. |

### src/util/memory.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 174 | RawHeapAccess | KEPT | `std::ptr::write_bytes` used for zeroing memory. |
| 188 | UncheckedCall | KEPT | `dzmmap` is unsafe by design to prevent overwriting existing mappings. |
| 248 | FFI | KEPT | `libc::mmap` call. |
| 266 | FFI | KEPT | `libc::prctl` call. |
| 288 | FFI | KEPT | `libc::madvise` call. |
| 302 | FFI | KEPT | `libc::munmap` call. |
| 384 | FFI | KEPT | `libc::mprotect` call. |
| 393 | FFI | KEPT | `libc::mprotect` call. |

### src/policy/space.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 371 | UncheckedCall | KEPT | `SFT_MAP.update` is unsafe because it modifies global SFT map. Caller must ensure validity. |
| 750 | UncheckedCall | KEPT | `sft_map.eager_initialize` is unsafe for same reason. |

### src/plan/global.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 114 | UncheckedCall | KEPT | `SFT_MAP.get_mut()` is used to initialize the global SFT map during single-threaded plan creation. |
| 761 | RawPointerCast | KEPT | `&*(self as *const CommonPlan<VM>)` used to pass a `'static` reference of `self` to a work packet. Safe as `Plan` is `'static`. |

### src/plan/barriers.rs
| Line | Category | Status | Notes |
|------|----------|--------|-------|
| 198 | UncheckedCall | KEPT | `S::UNLOG_BIT_SPEC.load` accesses side metadata (raw memory). |

## Refactoring Ideas
- Investigate if `Allocators` can be made safe by using a safe wrapper that checks initialization (if FFI allows).
- Explore zero-cost abstractions for `Address` that can encapsulate safety invariants where lifetimes can be proven.
- Investigate if `UnsafeCell` usage in `BlockQueue` can be replaced with safer concurrent primitives if performance allows.
- Investigate if `Prepare` work packet can use interior mutability for the parts of `Plan` it mutates, instead of casting away const.
