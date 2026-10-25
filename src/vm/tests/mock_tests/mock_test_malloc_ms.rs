use super::mock_test_prelude::*;

use crate::util::malloc::malloc_ms_util;

#[test]
fn test_malloc() {
    with_mockvm(
        default_setup,
        || {
            let cases = [
                (16usize, 8usize, 0usize, false),
                (16, 32, 0, false),
                (16, 8, 4, true),
                (32, 64, 4, true),
            ];
            for (size, align, offset, expect_offset_malloc) in cases {
                let (addr, is_offset_malloc) = malloc_ms_util::alloc::<MockVM>(size, align, offset);
                assert!(!addr.is_zero());
                assert!((addr + offset).is_aligned_to(align));
                assert_eq!(is_offset_malloc, expect_offset_malloc);

                // SAFETY: `addr` was asserted non-zero above and is a live allocation just
                // returned by `malloc_ms_util::alloc` paired with its `is_offset_malloc` flag; it
                // is freed once via the matching `offset_free`/`free` call immediately below.
                unsafe {
                    assert!(malloc_ms_util::get_malloc_usable_size(addr, is_offset_malloc) >= size);
                    if is_offset_malloc {
                        malloc_ms_util::offset_free(addr);
                    } else {
                        malloc_ms_util::free(addr.to_mut_ptr());
                    }
                }
            }
        },
        no_cleanup,
    )
}
