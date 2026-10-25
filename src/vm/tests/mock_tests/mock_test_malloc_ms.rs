use super::mock_test_prelude::*;

use crate::util::constants::BYTES_IN_ADDRESS;
use crate::util::malloc::{library, malloc_ms_util};
use crate::util::Address;

#[test]
fn test_malloc() {
    with_mockvm(
        default_setup,
        || {
            let cases = [
                (4usize, 16usize, 0usize, false),
                (8, 16, 0, false),
                (16, 8, 0, false),
                (16, 32, 0, false),
                (16, 4, 2, true),
                (8, 8, 2, true),
                (8, 16, 4, true),
                (8, 16, 16, true),
                (16, 8, 4, true),
                (32, 64, 4, true),
                (16, 128, 4, true),
            ];
            for (size, align, offset, expect_offset_malloc) in cases {
                let (addr, usable, is_offset_malloc) =
                    malloc_ms_util::alloc_with_usable_size(size, align, offset);
                assert!(!addr.is_zero());
                assert!((addr + offset).is_aligned_to(align));
                assert_eq!(is_offset_malloc, expect_offset_malloc);
                assert!(usable >= size);

                let (addr2, is_offset2) = malloc_ms_util::alloc(size, align, offset);
                assert!(!addr2.is_zero());
                assert!((addr2 + offset).is_aligned_to(align));
                assert_eq!(is_offset2, expect_offset_malloc);

                // SAFETY: `addr` and `addr2` are non-zero live allocations returned above paired
                // with their `is_offset_malloc` flags; when `is_offset_malloc` is true,
                // `addr - BYTES_IN_ADDRESS` holds the original base pointer from `calloc`. Both
                // allocations are freed once via `free_allocation` immediately below.
                unsafe {
                    assert_eq!(
                        malloc_ms_util::get_malloc_usable_size(addr, is_offset_malloc),
                        usable
                    );
                    if is_offset_malloc {
                        let raw = (addr - BYTES_IN_ADDRESS)
                            .to_ptr::<*mut libc::c_void>()
                            .read_unaligned();
                        let raw_end = Address::from_mut_ptr(raw).as_usize()
                            + library::malloc_usable_size(raw);
                        assert_eq!(
                            addr.as_usize() + usable,
                            raw_end,
                            "offset-malloc usable size must not overshoot raw allocation end"
                        );
                    }
                    malloc_ms_util::free_allocation(addr, is_offset_malloc);
                    assert!(malloc_ms_util::get_malloc_usable_size(addr2, is_offset2) >= size);
                    malloc_ms_util::free_allocation(addr2, is_offset2);
                }
            }
        },
        no_cleanup,
    )
}
