// GITHUB-CI: MMTK_PLAN=Immix
// GITHUB-CI: FEATURES=vm_space

// This test only runs for 64bits.
// It tries to set a certain range as VM space. The range does not conflict with the virtual
// address range we use for spaces. We cannot use SFTSpaceMap as the SFT map implementation.

use super::mock_test_prelude::*;
use crate::memory_manager;
use crate::util::Address;

#[test]
fn test_set_vm_space() {
    with_mockvm(
        default_setup,
        || {
            let _fixture = MMTKFixture::create_with_builder_and_mmtk_init(
                |_builder| {},
                true,
                |mmtk| {
                    // SAFETY: Hard-coded 64-bit chunk-aligned virtual address outside MMTk's
                    // default heap spaces, used only to register an external VM space range.
                    let start_addr = unsafe { Address::from_usize(0x78624DC00000) };
                    // SAFETY: Hard-coded 64-bit chunk-aligned end address (`> start_addr`) for the
                    // external VM space range; no memory is dereferenced.
                    let end_addr = unsafe { Address::from_usize(0x786258000000) };
                    let size = end_addr - start_addr;

                    memory_manager::set_vm_space::<MockVM>(mmtk, start_addr, size);
                },
            );
        },
        no_cleanup,
    )
}
