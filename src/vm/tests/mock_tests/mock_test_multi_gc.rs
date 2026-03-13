// Test: trigger multiple GC cycles with MockVM using the MarkSweep plan.
//
// # How to run
//
// ```
// MMTK_PLAN=MarkSweep cargo test \
//     --features 'mock_test immortal_as_nonmoving' \
//     mock_test_multi_gc -- --nocapture
// ```
//
// Required environment:
//   - MMTK_PLAN=MarkSweep   (auto-set by the test if unset, asserts if set to anything else)
//   - MMTK_THREADS=N        (optional, defaults to number of CPUs; 1 is useful for debugging)
//   - RUST_LOG=debug|trace  (optional, for scheduler/work-packet logging)
//
// Required features:
//   - mock_test               — enables MockVM and with_mockvm()
//   - immortal_as_nonmoving   — avoids ImmixSpace in CommonPlan's nonmoving space (see Pitfall 3)
//
// # Architecture
//
// This test runs a full MarkSweep GC cycle through the MMTk scheduler:
//   mutator allocates objects → trigger GC → ScheduleCollection → StopMutators →
//   Prepare → ScanMutatorRoots/ScanVMSpecificRoots → Closure → Release →
//   VMProcessWeakRefs → VMForwardWeakRefs → ResumeMutators
//
// # Pitfalls
//
// ## Pitfall 1: MockVM Mutex Deadlock
//
// The `mock!` macro acquires the global `MOCK_VM_INSTANCE` mutex on every VM call.
// If a mock callback blocks (e.g. waits on a condvar), the mutex stays held and
// any other thread calling ANY VM method will deadlock.
//
// **Consequence**: `block_for_gc` MUST be a no-op. Do NOT wait for GC completion
// inside any mock callback. Instead, use `trigger_gc_no_block()` to request GC
// without blocking, and synchronize externally via a separate condvar (`GcSync`).
// The `resume_mutators` mock signals this condvar.
//
// ## Pitfall 2: Worker TLS Must Be Non-Null
//
// The scheduler asserts `VMWorkerThread != UNINITIALIZED` in debug builds.
// MockVM must provide a valid (non-null) TLS for each worker thread:
//   Address::from_usize(0x1000 + ordinal * 8)
// Using VMThread::UNINITIALIZED (null) will panic in debug builds but silently
// pass in release builds, hiding the bug.
//
// ## Pitfall 3: CommonPlan's NonMovingSpace Defaults to ImmixSpace
//
// Without feature flags, `NonMovingSpace<VM>` = `ImmixSpace<VM>` (see plan/global.rs).
// ALL plans (including MarkSweep) include CommonPlan which has a nonmoving space.
// `ImmixSpace::prepare()` panics with "cyclic mark bits is not supported" when
// `LOCAL_MARK_BIT_SPEC` is in the object header (MockVM uses `in_header(0)`).
// ImmixSpace only supports side metadata for mark bits.
//
// **Fix**: Build with `--features immortal_as_nonmoving` to use ImmortalSpace instead.
// Alternatively, if MockVM's LOCAL_MARK_BIT_SPEC were changed to side metadata,
// this wouldn't be needed — but that would affect all other mock tests.
//
// ## Pitfall 4: mock_any! Boxes Factory Arguments
//
// `scan_roots_in_mutator_thread` and `scan_vm_specific_roots` in MockVM wrap the
// factory argument with `Box::new(factory)` before passing to `mock_any!()`:
//
//     fn scan_roots_in_mutator_thread(..., factory: impl RootsWorkFactory<...>) {
//         mock_any!(scan_roots_in_mutator_thread(tls, mutator, Box::new(factory)))
//     }
//
// Inside `mock_any!`, the arguments are boxed as `Box<dyn Any>` and later
// downcast::<I>() back. The type `I` must match EXACTLY. Since `Box::new(factory)`
// wraps `ProcessEdgesWorkRootsWorkFactory<...>` in a `Box`, the mock's declared
// input type must be `Box<MSFactory>`, NOT `MSFactory`.
//
// **Symptoms**: `mock_method.rs:16: called Result::unwrap() on an Err value: Any { .. }`
// This is the downcast failure, NOT a poisoned mutex (though a subsequent call
// will see a poisoned mutex from the first panic).
//
// ## Pitfall 5: Plan-Specific ProcessEdgesWork Types
//
// MockVM's `Box<dyn MockAny>` fields default to `SFTProcessEdges<MockVM>` as
// placeholder types (see mock_vm.rs comments). These placeholders WILL cause
// a downcast panic if the actual GC plan uses different ProcessEdgesWork types.
//
// MarkSweep's MSGCWorkContext uses:
//   DefaultProcessEdges = PlanProcessEdges<VM, MarkSweep<VM>, DEFAULT_TRACE>
//   PinningProcessEdges = PlanProcessEdges<VM, MarkSweep<VM>, DEFAULT_TRACE>
//
// ALL four Box<dyn MockAny> fields must be overridden with the correct types:
//   - scan_roots_in_mutator_thread: Box<ProcessEdgesWorkRootsWorkFactory<..., MSEdges, MSEdges>>
//   - scan_vm_specific_roots:       Box<ProcessEdgesWorkRootsWorkFactory<..., MSEdges, MSEdges>>
//   - process_weak_refs:            ProcessEdgesWorkTracerContext<MSEdges>
//   - forward_weak_refs:            ProcessEdgesWorkTracerContext<MSEdges>
//
// ## Pitfall 6: Mocks Required for a Full GC Cycle
//
// A complete MarkSweep GC cycle calls many VM trait methods. All of these must
// have working mocks (the defaults are `new_unimplemented()` and will panic):
//
//   Collection:   spawn_gc_thread, block_for_gc, stop_all_mutators, resume_mutators,
//                 is_collection_enabled, schedule_finalization
//   ActivePlan:   number_of_mutators, mutators (if needs_prepare_mutator=true)
//   Scanning:     scan_object, scan_roots_in_mutator_thread, scan_vm_specific_roots,
//                 notify_initial_thread_scan_complete, supports_return_barrier,
//                 prepare_for_roots_re_scanning, process_weak_refs, forward_weak_refs
//   ObjectModel:  get_object_size

use super::mock_test_prelude::*;
use crate::util::test_private::{
    trigger_gc_no_block, MarkSweep, PlanProcessEdges, ProcessEdgesWorkRootsWorkFactory,
    ProcessEdgesWorkTracerContext, DEFAULT_TRACE,
};
use crate::util::{Address, ObjectReference, OpaquePointer, VMThread, VMWorkerThread};
use crate::vm::slot::Slot;
use crate::AllocationSemantics;
use crate::Mutator;

use std::sync::{Arc, Condvar, Mutex};

const N_REFS: usize = 2;
const BYTES_PER_REF: usize = 8;
const OBJ_SIZE: usize = 8 + 8 + N_REFS * BYTES_PER_REF;

/// The ProcessEdgesWork type used by MarkSweep's MSGCWorkContext.
/// Other plans have different types — see Pitfall 5.
type MSEdges = PlanProcessEdges<MockVM, MarkSweep<MockVM>, DEFAULT_TRACE>;

/// The RootsWorkFactory type for scan_roots/scan_vm_specific_roots.
/// Note: mock_any! wraps this in Box, so mocks use Box<MSFactory> — see Pitfall 4.
type MSFactory = ProcessEdgesWorkRootsWorkFactory<MockVM, MSEdges, MSEdges>;

/// The TracerContext type for process_weak_refs/forward_weak_refs.
/// NOT boxed by mock_any! (passed directly).
type MSTracerCtx = ProcessEdgesWorkTracerContext<MSEdges>;

fn ref_field_addr(obj: ObjectReference, index: usize) -> Address {
    obj.to_raw_address() + BYTES_PER_REF + index * BYTES_PER_REF
}

static mut ROOT_NODES: Option<Vec<ObjectReference>> = None;
static mut MUTATOR_PTR: *mut Mutator<MockVM> = std::ptr::null_mut();

/// External GC synchronization — see Pitfall 1.
/// We cannot wait inside any mock callback (would deadlock MOCK_VM_INSTANCE mutex).
/// Instead, `resume_mutators` signals this condvar, and the test thread waits on it.
struct GcSync {
    inner: Mutex<bool>,
    condvar: Condvar,
}

impl GcSync {
    fn new() -> Self {
        GcSync {
            inner: Mutex::new(false),
            condvar: Condvar::new(),
        }
    }
    fn wait_for_gc(&self) {
        let mut done = self.inner.lock().unwrap();
        while !*done {
            done = self.condvar.wait(done).unwrap();
        }
        *done = false;
    }
    fn signal_gc_done(&self) {
        let mut done = self.inner.lock().unwrap();
        *done = true;
        self.condvar.notify_all();
    }
}

#[test]
pub fn test_multi_gc() {
    // Enforce MarkSweep plan — all Box<dyn MockAny> types are hardcoded to MSGCWorkContext.
    // See Pitfall 5 for why this is necessary.
    let plan = std::env::var("MMTK_PLAN").unwrap_or_default();
    assert!(
        plan.is_empty() || plan == "MarkSweep",
        "mock_test_multi_gc requires MMTK_PLAN=MarkSweep (got '{}'), \
         because the mock factory types are hardcoded to MarkSweep's GCWorkContext. \
         See Pitfall 5 in this file's documentation.",
        plan,
    );
    std::env::set_var("MMTK_PLAN", "MarkSweep");

    let gc_sync = Arc::new(GcSync::new());
    let gc_sync_resume = gc_sync.clone();

    with_mockvm(
        move || -> MockVM {
            MockVM {
                is_collection_enabled: MockMethod::new_fixed(Box::new(|_| true)),

                // Pitfall 2: worker TLS must be non-null.
                spawn_gc_thread: MockMethod::new_fixed(Box::new(|(_tls, ctx)| {
                    match ctx {
                        GCThreadContext::Worker(worker) => {
                            let ordinal = worker.ordinal;
                            let mmtk = worker.mmtk;
                            std::thread::spawn(move || {
                                let fake_tls_addr = unsafe { Address::from_usize(0x1000 + ordinal * 8) };
                                let worker_tls = VMWorkerThread(VMThread(OpaquePointer::from_address(fake_tls_addr)));
                                memory_manager::start_worker(mmtk, worker_tls, worker);
                            });
                        }
                    }
                })),

                // Pitfall 1: block_for_gc MUST be a no-op. Never wait here.
                block_for_gc: MockMethod::new_fixed(Box::new(move |_tls| {})),

                stop_all_mutators: MockMethod::new_fixed(Box::new(|(_tls, mut mutator_visitor)| {
                    let mutator = unsafe {
                        assert!(!std::ptr::addr_of!(MUTATOR_PTR).read().is_null());
                        &mut *std::ptr::addr_of!(MUTATOR_PTR).read()
                    };
                    mutator_visitor(mutator);
                })),

                // Pitfall 1: signal GC completion via external condvar.
                resume_mutators: MockMethod::new_fixed(Box::new(move |_tls| {
                    gc_sync_resume.signal_gc_done();
                })),

                // Pitfall 6: MarkSweep has needs_prepare_mutator=true, so Prepare::do_work
                // calls VMActivePlan::mutators() and number_of_mutators().
                number_of_mutators: MockMethod::new_fixed(Box::new(|_| 1)),
                mutators: MockMethod::new_fixed(Box::new(|_| {
                    let mutator = unsafe {
                        assert!(!std::ptr::addr_of!(MUTATOR_PTR).read().is_null());
                        &mut *std::ptr::addr_of!(MUTATOR_PTR).read()
                    };
                    Box::new(std::iter::once(mutator)) as Box<dyn Iterator<Item = &'static mut Mutator<MockVM>>>
                })),

                scan_object: MockMethod::new_fixed(Box::new(|(_tls, object, slot_visitor)| {
                    for r in 0..N_REFS {
                        let slot_addr = ref_field_addr(object, r);
                        if Slot::load(&slot_addr).is_some() {
                            slot_visitor.visit_slot(slot_addr);
                        }
                    }
                })),
                get_object_size: MockMethod::new_fixed(Box::new(|_| OBJ_SIZE)),

                // Pitfall 4 + 5: mock_any! boxes the factory, AND the type must match
                // MarkSweep's MSGCWorkContext (not SFTProcessEdges).
                scan_roots_in_mutator_thread: Box::new(MockMethod::<
                    (
                        VMWorkerThread,
                        &'static mut Mutator<MockVM>,
                        Box<MSFactory>,  // Pitfall 4: Box wrapping
                    ),
                    (),
                >::new_fixed(Box::new(|(_tls, _mutator, mut factory): (_, _, Box<MSFactory>)| {
                    let roots = unsafe { std::ptr::addr_of!(ROOT_NODES).as_ref().and_then(|r| r.as_ref()).cloned().unwrap_or_default() };
                    if !roots.is_empty() {
                        factory.create_process_pinning_roots_work(roots);
                    }
                }))),

                // Pitfall 4 + 5: same Box wrapping + MSEdges type.
                scan_vm_specific_roots: Box::new(MockMethod::<
                    (VMWorkerThread, Box<MSFactory>),  // Pitfall 4: Box wrapping
                    (),
                >::new_fixed(Box::new(|(_tls, _factory)| {}))),

                notify_initial_thread_scan_complete: MockMethod::new_fixed(Box::new(|_| {})),
                supports_return_barrier: MockMethod::new_fixed(Box::new(|_| false)),
                prepare_for_roots_re_scanning: MockMethod::new_fixed(Box::new(|_| {})),
                schedule_finalization: MockMethod::new_fixed(Box::new(|_| {})),

                // Pitfall 5: TracerContext type must match MSEdges, not SFTProcessEdges.
                // These are NOT boxed by mock_any! (unlike scan_roots).
                process_weak_refs: Box::new(MockMethod::<
                    (&'static mut crate::scheduler::GCWorker<MockVM>, MSTracerCtx),
                    bool,
                >::new_fixed(Box::new(|_| false))),
                forward_weak_refs: Box::new(MockMethod::<
                    (&'static mut crate::scheduler::GCWorker<MockVM>, MSTracerCtx),
                    (),
                >::new_fixed(Box::new(|_| {}))),

                ..MockVM::default()
            }
        },
        || {
            // No sleep needed between fixture creation and GC trigger.
            // initialize_collection() spawns workers asynchronously, but the scheduler
            // correctly queues GC requests: handle_user_collection_request() sets a flag,
            // and the first worker to park will process it via on_last_parked().
            let mut fixture = MutatorFixture::create_with_heapsize(4 * 1024 * 1024);
            unsafe { std::ptr::addr_of_mut!(MUTATOR_PTR).write(&mut *fixture.mutator as *mut _); }

            // Allocate objects and chain them: obj[0] -> obj[1] -> ... -> obj[9]
            let mut objects = Vec::new();
            for _ in 0..10 {
                let addr = memory_manager::alloc(&mut fixture.mutator, OBJ_SIZE, 8, 0, AllocationSemantics::Default);
                assert!(!addr.is_zero());
                let objref = MockVM::object_start_to_ref(addr);
                memory_manager::post_alloc(&mut fixture.mutator, objref, OBJ_SIZE, AllocationSemantics::Default);
                objects.push(objref);
            }
            for i in 0..objects.len() - 1 {
                Slot::store(&ref_field_addr(objects[i], 0), objects[i + 1]);
            }
            unsafe { std::ptr::addr_of_mut!(ROOT_NODES).write(Some(vec![objects[0]])); }

            let mmtk = fixture.mmtk();

            // Run 3 full GC cycles.
            // Uses trigger_gc_no_block() + gc_sync.wait_for_gc() — see Pitfall 1.
            for i in 0..3 {
                eprintln!("\n=== GC iteration {} ===", i);
                let did_gc = trigger_gc_no_block(mmtk);
                eprintln!("[test] GC {} triggered={}", i, did_gc);
                gc_sync.wait_for_gc();
                eprintln!("[test] GC {} done", i);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            eprintln!("\n=== All 3 GCs completed successfully ===");
        },
        no_cleanup,
    )
}
