// GITHUB-CI: MMTK_PLAN=Immix,SemiSpace,GenCopy,GenImmix,StickyImmix,ConcurrentImmix,MarkSweep
// GITHUB-CI: FEATURES=marksweep_as_nonmoving

use crate::memory_manager;
use crate::plan::AllocationSemantics;
use crate::util::constants::BYTES_IN_WORD;
use crate::util::test_util::fixtures::MutatorFixture;
use crate::util::test_util::mock_vm::{default_setup, no_cleanup, with_mockvm, MockVM};
use crate::util::ObjectReference;

#[test]
pub fn nonmoving_marksweep_nursery_and_full_heap_gc() {
    with_mockvm(
        default_setup,
        || {
            const MB: usize = 1024 * 1024;
            let mut fixture = MutatorFixture::create_with_heapsize(8 * MB);

            let initial_nonmoving: Vec<ObjectReference> = (0..32)
                .map(|_| {
                    let size = 64;
                    let addr = memory_manager::alloc(
                        fixture.mutator(),
                        size,
                        BYTES_IN_WORD,
                        0,
                        AllocationSemantics::NonMoving,
                    );
                    assert!(!addr.is_zero());
                    let obj = <MockVM>::object_start_to_ref(addr);
                    memory_manager::post_alloc(
                        fixture.mutator(),
                        obj,
                        size,
                        AllocationSemantics::NonMoving,
                    );
                    obj
                })
                .collect();

            // Allocate dead nursery/default objects (~4 MiB on an 8 MiB heap) to trigger nursery
            // GCs on generational plans (`GenCopy`, `GenImmix`, `StickyImmix`), verifying that
            // nursery GCs neither underflow `MarkSweepSpace::pending_release_packets` nor sweep
            // mature non-moving objects.
            for _ in 0..4096 {
                let size = 1024;
                let addr = memory_manager::alloc(
                    fixture.mutator(),
                    size,
                    BYTES_IN_WORD,
                    0,
                    AllocationSemantics::Default,
                );
                assert!(!addr.is_zero());
                let obj = <MockVM>::object_start_to_ref(addr);
                memory_manager::post_alloc(
                    fixture.mutator(),
                    obj,
                    size,
                    AllocationSemantics::Default,
                );
            }

            #[cfg(feature = "vo_bit")]
            if fixture.mmtk().get_plan().generational().is_some()
                && !fixture.mmtk().get_plan().last_collection_was_exhaustive()
            {
                for &obj in &initial_nonmoving {
                    assert!(
                        crate::util::metadata::vo_bit::is_vo_bit_set(obj),
                        "Nursery GC must not sweep mature non-moving object {}",
                        obj
                    );
                }
            }
            let _ = &initial_nonmoving;

            // Trigger an explicit exhaustive full-heap GC and verify dead non-moving objects are
            // swept and their MarkSweep blocks are recycled across repeated allocation rounds
            // totaling 12 MiB on an 8 MiB heap.
            fixture
                .mmtk()
                .handle_user_collection_request(fixture.mutator_tls(), true, true);

            #[cfg(feature = "vo_bit")]
            for &obj in &initial_nonmoving {
                assert!(
                    !crate::util::metadata::vo_bit::is_vo_bit_set(obj),
                    "Full-heap GC must sweep unmarked non-moving object {}",
                    obj
                );
            }

            for _round in 0..6 {
                for _ in 0..2048 {
                    let size = 1024;
                    let addr = memory_manager::alloc(
                        fixture.mutator(),
                        size,
                        BYTES_IN_WORD,
                        0,
                        AllocationSemantics::NonMoving,
                    );
                    assert!(!addr.is_zero());
                    let obj = <MockVM>::object_start_to_ref(addr);
                    memory_manager::post_alloc(
                        fixture.mutator(),
                        obj,
                        size,
                        AllocationSemantics::NonMoving,
                    );
                }
                fixture
                    .mmtk()
                    .handle_user_collection_request(fixture.mutator_tls(), true, true);
            }
        },
        no_cleanup,
    );
}
