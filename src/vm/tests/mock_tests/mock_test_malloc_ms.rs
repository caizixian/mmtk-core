#![deny(unsafe_op_in_unsafe_fn)]

use super::mock_test_prelude::*;

use crate::util::malloc::malloc_ms_util;

#[test]
fn test_malloc() {
    with_mockvm(
        default_setup,
        || {
            let (address1, bool1) = malloc_ms_util::alloc::<MockVM>(16, 8, 0);
            let (address2, bool2) = malloc_ms_util::alloc::<MockVM>(16, 32, 0);
            let (address3, bool3) = malloc_ms_util::alloc::<MockVM>(16, 8, 4);
            let (address4, bool4) = malloc_ms_util::alloc::<MockVM>(32, 64, 4);

            assert!(!address1.is_zero());
            assert!(!address2.is_zero());
            assert!(!address3.is_zero());
            assert!(!address4.is_zero());

            assert!(address1.is_aligned_to(8));
            assert!(address2.is_aligned_to(32));
            assert!((address3 + 4_isize).is_aligned_to(8));
            assert!((address4 + 4_isize).is_aligned_to(64));

            assert!(!bool1);
            assert!(!bool2);

            assert!(bool3);
            assert!(bool4);

            // SAFETY: `address1`..`address4` were asserted non-zero above and are live allocations
            // returned by `malloc_ms_util::alloc` paired with their exact `is_offset_malloc` flags
            // `bool1`..`bool4`.
            assert!(unsafe { malloc_ms_util::get_malloc_usable_size(address1, bool1) } >= 16);
            // SAFETY: `address2` is a non-zero live allocation from `alloc` paired with `bool2`.
            assert!(unsafe { malloc_ms_util::get_malloc_usable_size(address2, bool2) } >= 16);
            // SAFETY: `address3` is a non-zero live offset allocation from `alloc` paired with `bool3`.
            assert!(unsafe { malloc_ms_util::get_malloc_usable_size(address3, bool3) } >= 16);
            // SAFETY: `address4` is a non-zero live offset allocation from `alloc` paired with `bool4`.
            assert!(unsafe { malloc_ms_util::get_malloc_usable_size(address4, bool4) } >= 32);

            // SAFETY: `address1` was returned directly from `malloc_ms_util::alloc` without an
            // alignment offset (`!bool1`), is non-null, and has not yet been freed.
            unsafe {
                malloc_ms_util::free(address1.to_mut_ptr());
            }
            // SAFETY: `address2` was returned directly from `malloc_ms_util::alloc` without an
            // alignment offset (`!bool2`), is non-null, and has not yet been freed.
            unsafe {
                malloc_ms_util::free(address2.to_mut_ptr());
            }
            // SAFETY: `address3` is a non-zero live allocation returned by `malloc_ms_util::alloc`
            // with `bool3 == true` (`align_offset_alloc`), and is freed once here.
            unsafe {
                malloc_ms_util::offset_free(address3);
            }
            // SAFETY: `address4` is a non-zero live allocation returned by `malloc_ms_util::alloc`
            // with `bool4 == true` (`align_offset_alloc`), and is freed once here.
            unsafe {
                malloc_ms_util::offset_free(address4);
            }
        },
        no_cleanup,
    )
}
