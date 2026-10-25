// GITHUB-CI: MMTK_PLAN=all

use super::mock_test_prelude::*;
use super::mock_test_vm_layout_default::test_with_vm_layout;
use crate::util::heap::vm_layout::VMLayout;
use crate::util::Address;

#[test]
fn test_vm_layout_heap_start() {
    with_mockvm(
        default_setup,
        || {
            let default = VMLayout::default();

            // Test with a start address that is different from the default heap start.
            #[cfg(target_pointer_width = "32")]
            const HEAP_START_RAW: usize = 0x7000_0000;
            #[cfg(target_pointer_width = "64")]
            const HEAP_START_RAW: usize = 0x0000_0400_0000_0000;

            // SAFETY: Hard-coded aligned virtual address constant for `VMLayout::heap_start`;
            // MMTk maps heap chunks using non-replacing mmap without dereferencing unmapped memory.
            let heap_start = unsafe { Address::from_usize(HEAP_START_RAW) };
            #[cfg(target_pointer_width = "64")]
            assert!(heap_start.is_aligned_to(default.max_space_extent()));

            let layout = VMLayout {
                heap_start,
                // Use default for the rest.
                ..default
            };
            test_with_vm_layout(Some(layout));
        },
        no_cleanup,
    )
}
