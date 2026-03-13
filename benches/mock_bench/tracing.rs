//! Mock benchmarks for the object graph tracing loop using the real MMTk scheduler.
//!
//! # How to run
//!
//! ```sh
//! MMTK_BENCH=tracing MMTK_PLAN=MarkSweep cargo bench \
//!     --features 'mock_test immortal_as_nonmoving'
//! ```
//!
//! Required env:
//!   - MMTK_BENCH=tracing      (selects this benchmark in mock_bench/mod.rs)
//!   - MMTK_PLAN=MarkSweep     (auto-set; see Pitfall 5 in mock_test_multi_gc.rs)
//!
//! Required features:
//!   - mock_test                (enables MockVM)
//!   - immortal_as_nonmoving    (avoids ImmixSpace cyclic mark bits; see Pitfall 3)
//!
//! Optional env:
//!   - MMTK_THREADS=N           (default: num CPUs)
//!   - RUST_LOG=debug|trace     (scheduler logging)
//!
//! # Architecture
//!
//! Each benchmark iteration triggers one full MarkSweep GC cycle through the scheduler:
//!   ScheduleCollection → StopMutators → Prepare → ScanMutatorRoots/ScanVMSpecificRoots →
//!   Closure (ProcessEdgesWork → ScanObjectsWork → ...) → Release → ResumeMutators
//!
//! GC workers stay alive between iterations (spawned once by initialize_collection).
//! The tracing work goes through the real work packet pipeline.
//!
//! # Pitfalls
//!
//! This benchmark applies ALL the same pitfalls documented in mock_test_multi_gc.rs:
//!   - Pitfall 1: mock! mutex deadlock — block_for_gc must be a no-op
//!   - Pitfall 2: worker TLS must be non-null
//!   - Pitfall 3: immortal_as_nonmoving feature required
//!   - Pitfall 4: mock_any! boxes factory arguments
//!   - Pitfall 5: SFT → PlanProcessEdges type mismatch
//!   - Pitfall 6: all mocks needed for full GC cycle
//! See that file for detailed explanations.

use criterion::Criterion;
use std::sync::{Arc, Condvar, Mutex};

use mmtk::memory_manager;
use mmtk::util::test_private::{
    trigger_gc_no_block, MarkSweep, PlanProcessEdges, ProcessEdgesWorkRootsWorkFactory,
    ProcessEdgesWorkTracerContext, DEFAULT_TRACE,
};
use mmtk::util::test_util::fixtures::*;
use mmtk::util::test_util::mock_method::*;
use mmtk::util::test_util::mock_vm::{write_mockvm, MockVM};
use mmtk::util::{Address, ObjectReference, VMThread, VMWorkerThread};
use mmtk::vm::slot::Slot;
use mmtk::vm::GCThreadContext;
use mmtk::vm::RootsWorkFactory;
use mmtk::AllocationSemantics;
use mmtk::Mutator;

use mmtk::util::OpaquePointer;

const BYTES_PER_REF: usize = std::mem::size_of::<usize>();
/// Number of reference fields per object.
const N_REFS: usize = 4;
/// Object size: 8 (pre-header) + 8 (header word for mark bits) + N_REFS * 8 (ref fields)
const OBJ_SIZE: usize = 8 + 8 + N_REFS * BYTES_PER_REF;

/// The ProcessEdgesWork type used by MarkSweep's MSGCWorkContext.
type MSEdges = PlanProcessEdges<MockVM, MarkSweep<MockVM>, DEFAULT_TRACE>;
/// The RootsWorkFactory type — mock_any! wraps in Box (Pitfall 4).
type MSFactory = ProcessEdgesWorkRootsWorkFactory<MockVM, MSEdges, MSEdges>;
/// The TracerContext type for process_weak_refs/forward_weak_refs (NOT boxed).
type MSTracerCtx = ProcessEdgesWorkTracerContext<MSEdges>;

/// Get the address of the i-th reference field in an object.
/// Fields start after the header word (at obj_ref + 8).
#[inline(always)]
fn ref_field_addr(obj: ObjectReference, index: usize) -> Address {
    obj.to_raw_address() + BYTES_PER_REF + index * BYTES_PER_REF
}

/// Store an ObjectReference into a reference field of an object.
#[inline(always)]
fn store_ref_field(obj: ObjectReference, index: usize, target: ObjectReference) {
    Slot::store(&ref_field_addr(obj, index), target);
}

/// Allocate `count` objects, each OBJ_SIZE bytes.
fn allocate_objects(fixture: &mut MutatorFixture, count: usize) -> Vec<ObjectReference> {
    let mut objects = Vec::with_capacity(count);
    for _ in 0..count {
        let addr = memory_manager::alloc(
            &mut fixture.mutator,
            OBJ_SIZE,
            8,
            0,
            AllocationSemantics::Default,
        );
        assert!(!addr.is_zero(), "Allocation failed");
        let objref = MockVM::object_start_to_ref(addr);
        memory_manager::post_alloc(
            &mut fixture.mutator,
            objref,
            OBJ_SIZE,
            AllocationSemantics::Default,
        );
        objects.push(objref);
    }
    objects
}

/// Build a random DAG: each object has N_REFS references to random other objects.
fn build_random_dag(objects: &[ObjectReference]) {
    use rand::rngs::SmallRng; // SmallRng = Xoshiro256PlusPlus on 64-bit
    use rand::RngExt;
    use rand::SeedableRng;

    let n = objects.len();
    let mut rng = SmallRng::seed_from_u64(0xdeadbeef12345678);
    for i in 0..n {
        for r in 0..N_REFS {
            let target = rng.random_range(0..n);
            store_ref_field(objects[i], r, objects[target]);
        }
    }
}

/// GC synchronization — Pitfall 1: NEVER wait inside a mock callback.
/// `resume_mutators` signals this condvar; the benchmark thread waits on it externally.
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

// Global state shared between benchmark and MockVM callbacks.
// Safety: these are set before the GC cycle starts and read during (single-writer).
static mut ROOT_NODES: Option<Vec<ObjectReference>> = None;
static mut MUTATOR_PTR: *mut Mutator<MockVM> = std::ptr::null_mut();

pub fn bench(c: &mut Criterion) {
    // Enforce MarkSweep — all Box<dyn MockAny> types are hardcoded to MSGCWorkContext.
    std::env::set_var("MMTK_PLAN", "MarkSweep");

    let gc_sync = Arc::new(GcSync::new());
    let gc_sync_resume = gc_sync.clone();

    write_mockvm(|mock| {
        *mock = MockVM {
            is_collection_enabled: MockMethod::new_fixed(Box::new(|_| true)),

            // Pitfall 2: worker TLS must be non-null.
            spawn_gc_thread: MockMethod::new_fixed(Box::new(|(_tls, ctx)| {
                match ctx {
                    GCThreadContext::Worker(worker) => {
                        let ordinal = worker.ordinal;
                        let mmtk = worker.mmtk;
                        std::thread::spawn(move || {
                            let fake_tls_addr =
                                unsafe { Address::from_usize(0x1000 + ordinal * 8) };
                            let worker_tls = VMWorkerThread(VMThread(
                                OpaquePointer::from_address(fake_tls_addr),
                            ));
                            memory_manager::start_worker(mmtk, worker_tls, worker);
                        });
                    }
                }
            })),

            // Pitfall 1: block_for_gc MUST be a no-op. Never wait here.
            block_for_gc: MockMethod::new_fixed(Box::new(move |_tls| {})),

            stop_all_mutators: MockMethod::new_fixed(Box::new(|(_tls, mut mutator_visitor)| {
                let mutator = unsafe {
                    assert!(!std::ptr::addr_of!(MUTATOR_PTR).read().is_null(), "MUTATOR_PTR not set");
                    &mut *std::ptr::addr_of!(MUTATOR_PTR).read()
                };
                mutator_visitor(mutator);
            })),

            // Pitfall 1: signal GC completion via external condvar.
            resume_mutators: MockMethod::new_fixed(Box::new(move |_tls| {
                gc_sync_resume.signal_gc_done();
            })),

            // Pitfall 6: MarkSweep has needs_prepare_mutator=true.
            number_of_mutators: MockMethod::new_fixed(Box::new(|_| 1)),
            mutators: MockMethod::new_fixed(Box::new(|_| {
                let mutator = unsafe {
                    assert!(!std::ptr::addr_of!(MUTATOR_PTR).read().is_null());
                    &mut *std::ptr::addr_of!(MUTATOR_PTR).read()
                };
                Box::new(std::iter::once(mutator))
                    as Box<dyn Iterator<Item = &'static mut Mutator<MockVM>>>
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

            // Pitfall 4 + 5: Box-wrapped MSFactory, not SFTProcessEdges.
            scan_roots_in_mutator_thread: Box::new(MockMethod::<
                (
                    VMWorkerThread,
                    &'static mut Mutator<MockVM>,
                    Box<MSFactory>,
                ),
                (),
            >::new_fixed(Box::new(
                |(_tls, _mutator, mut factory): (_, _, Box<MSFactory>)| {
                    let roots = unsafe { std::ptr::addr_of!(ROOT_NODES).as_ref().and_then(|r| r.as_ref()).cloned().unwrap_or_default() };
                    if !roots.is_empty() {
                        factory.create_process_pinning_roots_work(roots);
                    }
                },
            ))),

            scan_vm_specific_roots: Box::new(MockMethod::<
                (VMWorkerThread, Box<MSFactory>),
                (),
            >::new_fixed(Box::new(|(_tls, _factory)| {}))),

            notify_initial_thread_scan_complete: MockMethod::new_fixed(Box::new(|_| {})),
            supports_return_barrier: MockMethod::new_fixed(Box::new(|_| false)),
            prepare_for_roots_re_scanning: MockMethod::new_fixed(Box::new(|_| {})),
            schedule_finalization: MockMethod::new_fixed(Box::new(|_| {})),

            // Pitfall 5: TracerContext must use MSEdges, not SFTProcessEdges.
            process_weak_refs: Box::new(MockMethod::<
                (&'static mut mmtk::scheduler::GCWorker<MockVM>, MSTracerCtx),
                bool,
            >::new_fixed(Box::new(|_| false))),
            forward_weak_refs: Box::new(MockMethod::<
                (&'static mut mmtk::scheduler::GCWorker<MockVM>, MSTracerCtx),
                (),
            >::new_fixed(Box::new(|_| {}))),

            ..MockVM::default()
        };
    });

    // --- Setup: allocate graph ---
    let n_objects: usize = 16384;
    let heap_size = n_objects * OBJ_SIZE * 4; // generous heap

    let mut fixture = MutatorFixture::create_with_heapsize(heap_size);

    unsafe {
        std::ptr::addr_of_mut!(MUTATOR_PTR).write(&mut *fixture.mutator as *mut _);
    }

    let objects = allocate_objects(&mut fixture, n_objects);
    build_random_dag(&objects);

    // Set root: first object (graph is connected via random DAG)
    unsafe {
        std::ptr::addr_of_mut!(ROOT_NODES).write(Some(vec![objects[0]]));
    }

    let mmtk = fixture.mmtk();

    c.bench_function(&format!("trace_gc_random_dag_{}", n_objects), |b| {
        b.iter(|| {
            // Each iteration triggers one full GC cycle through the real scheduler.
            // trigger_gc_no_block: Pitfall 1 — don't call block_for_gc.
            let did_gc = trigger_gc_no_block(mmtk);
            gc_sync.wait_for_gc();
            std::hint::black_box(did_gc);
        });
    });
}
