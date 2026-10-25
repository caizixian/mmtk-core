// GITHUB-CI: MMTK_PLAN=Immix,SemiSpace,GenCopy,GenImmix,StickyImmix,ConcurrentImmix,MarkSweep,Lisp2
// GITHUB-CI: FEATURES=marksweep_as_nonmoving

use std::collections::HashSet;

use crate::memory_manager;
use crate::plan::AllocationSemantics;
use crate::util::constants::BYTES_IN_WORD;
use crate::util::test_util::fixtures::MutatorFixture;
use crate::util::test_util::mock_vm::{no_cleanup, with_mockvm, MockMethod, MockMethodFn, MockVM};
use crate::util::ObjectReference;

#[test]
pub fn nonmoving_marksweep_nursery_and_full_heap_gc() {
    const OBJECT_SIZE: usize = 1024;
    with_mockvm(
        || MockVM {
            get_object_size: MockMethod::new_fixed(Box::new(|_| OBJECT_SIZE)),
            get_object_size_when_copied: MockMethod::new_fixed(Box::new(|_| OBJECT_SIZE)),
            scan_object: MockMethodFn::new_fixed(Box::new(|_, _, _| {})),
            scan_object_and_trace_edges: MockMethodFn::new_fixed(Box::new(|_, _, _| {})),
            ..MockVM::default()
        },
        || {
            const MB: usize = 1024 * 1024;
            let mut fixture = MutatorFixture::create_with_heapsize(8 * MB);

            let alloc_nonmoving = |fixture: &mut MutatorFixture| -> ObjectReference {
                let addr = memory_manager::alloc(
                    fixture.mutator(),
                    OBJECT_SIZE,
                    BYTES_IN_WORD,
                    0,
                    AllocationSemantics::NonMoving,
                );
                assert!(!addr.is_zero());
                let obj = <MockVM>::object_start_to_ref(addr);
                memory_manager::post_alloc(
                    fixture.mutator(),
                    obj,
                    OBJECT_SIZE,
                    AllocationSemantics::NonMoving,
                );
                obj
            };

            // Keep 16 non-moving objects live across GC cycles via FinalizableProcessor (retained in
            // `ready_for_finalize` and traced in both Marking and Forwarding passes until popped).
            let rooted_nonmoving: Vec<ObjectReference> = (0..16)
                .map(|_| {
                    let obj = alloc_nonmoving(&mut fixture);
                    memory_manager::add_finalizer(fixture.mmtk(), obj);
                    obj
                })
                .collect();
            let rooted_set: HashSet<ObjectReference> = rooted_nonmoving.iter().copied().collect();

            let unrooted_nonmoving: Vec<ObjectReference> =
                (0..16).map(|_| alloc_nonmoving(&mut fixture)).collect();

            // Allocate dead nursery/default objects (4.5 MiB on an 8 MiB heap) to cross the 50%
            // concurrent-marking threshold on `ConcurrentImmix` (verifying concurrent marking stays
            // disabled under `marksweep_as_nonmoving`) and trigger nursery GCs on generational plans
            // (`GenCopy`, `GenImmix`, `StickyImmix`), verifying nursery GCs neither underflow
            // `MarkSweepSpace::pending_release_packets` nor sweep mature non-moving objects.
            for _ in 0..4608 {
                let addr = memory_manager::alloc(
                    fixture.mutator(),
                    OBJECT_SIZE,
                    BYTES_IN_WORD,
                    0,
                    AllocationSemantics::Default,
                );
                assert!(!addr.is_zero());
                let obj = <MockVM>::object_start_to_ref(addr);
                memory_manager::post_alloc(
                    fixture.mutator(),
                    obj,
                    OBJECT_SIZE,
                    AllocationSemantics::Default,
                );
            }

            assert!(
                fixture
                    .mmtk()
                    .get_plan()
                    .concurrent()
                    .is_none_or(|c| !c.concurrent_work_in_progress()),
                "Concurrent marking must remain disabled when marksweep_as_nonmoving is enabled"
            );

            #[cfg(feature = "vo_bit")]
            if fixture.mmtk().get_plan().generational().is_some()
                && !fixture.mmtk().get_plan().last_collection_was_exhaustive()
            {
                for &obj in rooted_nonmoving.iter().chain(&unrooted_nonmoving) {
                    assert!(
                        crate::util::metadata::vo_bit::is_vo_bit_set(obj),
                        "Nursery GC must not sweep mature non-moving object {}",
                        obj
                    );
                }
            }

            // Trigger an explicit exhaustive full-heap GC and verify unrooted non-moving objects are
            // swept while rooted non-moving objects stay live and unaliased across 12 MiB of churn.
            fixture
                .mmtk()
                .handle_user_collection_request(fixture.mutator_tls(), true, true);

            for &obj in &rooted_nonmoving {
                assert!(
                    obj.is_live(),
                    "Rooted non-moving object {obj} must stay live"
                );
                #[cfg(feature = "vo_bit")]
                assert!(
                    crate::util::metadata::vo_bit::is_vo_bit_set(obj),
                    "Full-heap GC must retain VO bit on live non-moving object {}",
                    obj
                );
            }
            for &obj in &unrooted_nonmoving {
                assert!(
                    !obj.is_live(),
                    "Unrooted non-moving object {obj} must not stay marked"
                );
                #[cfg(feature = "vo_bit")]
                assert!(
                    !crate::util::metadata::vo_bit::is_vo_bit_set(obj),
                    "Full-heap GC must sweep unmarked non-moving object {}",
                    obj
                );
            }

            for _round in 0..6 {
                for _ in 0..2048 {
                    let obj = alloc_nonmoving(&mut fixture);
                    assert!(
                        !rooted_set.contains(&obj),
                        "Live rooted non-moving cell {obj} was swept and reallocated"
                    );
                }
                fixture
                    .mmtk()
                    .handle_user_collection_request(fixture.mutator_tls(), true, true);
                for &obj in &rooted_nonmoving {
                    assert!(
                        obj.is_live(),
                        "Rooted non-moving object {obj} must survive across repeated full-heap GCs"
                    );
                    #[cfg(feature = "vo_bit")]
                    assert!(crate::util::metadata::vo_bit::is_vo_bit_set(obj));
                }
            }

            // Pop all retained finalizable references and verify the next full-heap GC sweeps them.
            let mut popped = 0;
            while memory_manager::get_finalized_object(fixture.mmtk()).is_some() {
                popped += 1;
            }
            assert_eq!(popped, rooted_nonmoving.len());
            fixture
                .mmtk()
                .handle_user_collection_request(fixture.mutator_tls(), true, true);
            for &obj in &rooted_nonmoving {
                assert!(
                    !obj.is_live(),
                    "Unrooted non-moving object {obj} must be unmarked after finalizer pop"
                );
                #[cfg(feature = "vo_bit")]
                assert!(
                    !crate::util::metadata::vo_bit::is_vo_bit_set(obj),
                    "Unrooted non-moving object {obj} must have VO bit cleared after sweep"
                );
            }
        },
        no_cleanup,
    );
}
