//! Microbenchmark for software prefetching in the GC tracing loop.
//!
//! # How to run
//!
//! ```sh
//! MMTK_BENCH=prefetch_tracing MMTK_PLAN=MarkSweep cargo bench \
//!     --features 'mock_test immortal_as_nonmoving'
//! ```
//!
//! # What it measures
//!
//! This benchmark simulates MMTk's tracing loop as a BFS graph traversal with
//! a work queue of slot batches (work packets).  It measures the effect of
//! software prefetching at various distances on the slot-processing inner loop.
//!
//! ## Modelling the real tracing loop
//!
//! In MMTk, graph tracing is an iterative BFS driven by work packets:
//!
//!   1. **Roots** produce an initial batch of slots (a `ProcessEdgesWork` packet).
//!   2. **`ProcessEdgesWork::process_slots()`** processes up to `PACKET_SIZE` slots:
//!      - For each slot: load objref, test-and-mark.
//!      - If newly marked: push objref to a `nodes` buffer.
//!   3. **`ScanObjects::do_work()`** scans the `nodes` buffer:
//!      - For each object: load klass pointer, iterate OopMaps.
//!      - Each reference field produces a new slot → accumulated into a new
//!        `ProcessEdgesWork` packet.
//!   4. New packets are added to the work queue → repeat from step 2.
//!
//! In this benchmark, we combine steps 2+3 into a single loop for simplicity
//! (i.e., when we newly mark an object, we immediately scan it to produce new
//! slots into a local buffer, which then becomes the next work packet).  This
//! simulates `SCAN_OBJECTS_IMMEDIATELY=true`, which is the default for most
//! plans.
//!
//! The BFS traversal processes the entire live object graph starting from
//! root objects, with work generated dynamically — NOT from a pre-built array.
//!
//! ## Object layout modelling OpenJDK
//!
//! Each object has:
//!   - Header word (mark bits) at `obj_ref + 0`
//!   - Klass pointer at `obj_ref + 8` → points to a `KlassInfo` struct
//!     allocated in a separate buffer (causes real cache misses)
//!   - `N_REFS` reference fields starting at `obj_ref + 16`
//!
//! Scanning an object requires loading the klass pointer (dependent cache miss),
//! reading OopMap metadata (n_refs, field offsets), then iterating field addresses.
//! This per-object work makes prefetch distance meaningful.
//!
//! ## Prefetching targets (from Atkinson 2023 taxonomy)
//!
//!   1. **Edge prefetching** — prefetch the slot content (the objref) ahead of loading
//!   2. **Object prefetching** — prefetch the object header (mark word + klass ptr)
//!
//! ## Key findings from prior work
//!
//!   - Huang 2025: edge=32, object=16, NTA → 9% (Coffee Lake) / 18% (Zen 4) GC speedup
//!   - Atkinson 2023: PREFETCHT0 gives +0.5–3.1% over NTA on Zen 4
//!   - Huang 2025: only 1.85% remaining headroom after prefetching

use criterion::{black_box, Criterion};
use std::collections::VecDeque;
use std::time::Duration;

use rand::rngs::SmallRng;
use rand::RngExt;
use rand::SeedableRng;

use mmtk::memory_manager;
use mmtk::util::test_util::fixtures::*;
use mmtk::util::test_util::mock_method::*;
use mmtk::util::test_util::mock_vm::{write_mockvm, MockVM};
use mmtk::util::{Address, ObjectReference};
use mmtk::vm::slot::Slot;
use mmtk::AllocationSemantics;

const BYTES_PER_REF: usize = std::mem::size_of::<usize>();

// ───────────────────────────────────────────────────────────────────────────
// Object layout
// ───────────────────────────────────────────────────────────────────────────

/// Number of reference fields per object.
const N_REFS: usize = 4;

/// Padding bytes per object to ensure the heap well exceeds LLC.
/// AMD EPYC 7B13 (dual-socket): 32 MB L3 per CCD × 4 CCDs/socket × 2 sockets
/// = 256 MB total L3, but each CCD's L3 is separate (not unified).
/// A single-threaded benchmark sees only the local CCD's 32 MB L3.
/// With 4M objects × 256 bytes = 1 GB heap → ~32× per-CCD L3.
const OBJ_PADDING: usize = 200;

/// Object size: 8 (pre-header) + 8 (header/mark) + 8 (klass_ptr) + N_REFS * 8 + padding
const OBJ_SIZE: usize = 8 + 8 + 8 + N_REFS * BYTES_PER_REF + OBJ_PADDING;

/// Work packet size — matches MMTk's EDGES_WORK_BUFFER_SIZE = 4096.
const PACKET_SIZE: usize = 4096;

/// Simulated klass information — models OpenJDK's InstanceKlass + OopMapBlock.
#[repr(C)]
struct KlassInfo {
    n_refs: usize,
    offsets: [usize; N_REFS],
}

#[inline(always)]
fn klass_ptr_addr(obj: ObjectReference) -> Address {
    obj.to_raw_address() + BYTES_PER_REF
}

#[inline(always)]
fn ref_field_addr(obj: ObjectReference, index: usize) -> Address {
    obj.to_raw_address() + 2 * BYTES_PER_REF + index * BYTES_PER_REF
}

#[inline(always)]
fn store_ref_field(obj: ObjectReference, index: usize, target: ObjectReference) {
    Slot::store(&ref_field_addr(obj, index), target);
}

#[inline(always)]
fn read_header(obj: ObjectReference) -> usize {
    unsafe { obj.to_raw_address().load::<usize>() }
}

#[inline(always)]
fn write_header(obj: ObjectReference, val: usize) {
    unsafe { obj.to_raw_address().store::<usize>(val) }
}

#[inline(always)]
fn load_klass(obj: ObjectReference) -> *const KlassInfo {
    unsafe { klass_ptr_addr(obj).load::<usize>() as *const KlassInfo }
}

fn clear_marks(objects: &[ObjectReference]) {
    for &obj in objects {
        let header = read_header(obj);
        write_header(obj, header & !1);
    }
}

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

fn build_random_dag_with_klass(
    objects: &[ObjectReference],
    klass_storage: &mut Vec<KlassInfo>,
) {
    let n = objects.len();
    let mut rng = SmallRng::seed_from_u64(0xdeadbeef12345678);

    let field_offsets: [usize; N_REFS] = std::array::from_fn(|i| {
        2 * BYTES_PER_REF + i * BYTES_PER_REF
    });

    klass_storage.clear();
    klass_storage.reserve(n);
    for _ in 0..n {
        klass_storage.push(KlassInfo {
            n_refs: N_REFS,
            offsets: field_offsets,
        });
    }

    for i in 0..n {
        let klass_ptr = &klass_storage[i] as *const KlassInfo as usize;
        unsafe {
            klass_ptr_addr(objects[i]).store::<usize>(klass_ptr);
        }
        for r in 0..N_REFS {
            let target = rng.random_range(0..n);
            store_ref_field(objects[i], r, objects[target]);
        }
    }
}

#[inline(always)]
fn prefetch_nta(addr: Address) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        std::arch::x86_64::_mm_prefetch(
            addr.to_ptr::<i8>(),
            std::arch::x86_64::_MM_HINT_NTA,
        );
    }
}

#[inline(always)]
fn prefetch_t0(addr: Address) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        std::arch::x86_64::_mm_prefetch(
            addr.to_ptr::<i8>(),
            std::arch::x86_64::_MM_HINT_T0,
        );
    }
}

// ───────────────────────────────────────────────────────────────────────────
// BFS graph tracing with work queue
// ───────────────────────────────────────────────────────────────────────────

/// A slot is just an Address (matching MMTk's SimpleSlot = Address).
type BenchSlot = Address;

/// Process a single slot: load objref → test-and-mark → if newly marked,
/// scan object and push child slots into `new_slots`.
///
/// This combines ProcessEdgesWork::process_slot + ScanObjects scanning,
/// modelling SCAN_OBJECTS_IMMEDIATELY = true.
#[inline(always)]
fn process_and_scan_slot(slot: BenchSlot, new_slots: &mut Vec<BenchSlot>) {
    let objref: Option<ObjectReference> = Slot::load(&slot);
    if let Some(obj) = objref {
        let header = read_header(obj);
        if header & 1 == 0 {
            // Newly marked — set mark bit
            write_header(obj, header | 1);
            // Scan object: load klass, iterate OopMaps → produce child slots
            let klass = load_klass(obj);
            let klass_info = unsafe { &*klass };
            for f in 0..klass_info.n_refs {
                let child_slot = obj.to_raw_address() + klass_info.offsets[f];
                new_slots.push(child_slot);
            }
        }
    }
}

/// BFS tracing strategy: no prefetching (baseline).
/// Processes work packets of `packet_size` slots, generating new packets dynamically.
/// Returns total number of marked objects.
fn trace_bfs_baseline(roots: &[BenchSlot], packet_size: usize) -> usize {
    let mut work_queue: VecDeque<Vec<BenchSlot>> = VecDeque::new();

    // Seed the queue with the root slots, split into packets
    for chunk in roots.chunks(packet_size) {
        work_queue.push_back(chunk.to_vec());
    }

    let mut marked_count = 0usize;
    let mut new_slots = Vec::with_capacity(packet_size * 2);

    while let Some(packet) = work_queue.pop_front() {
        // Process one work packet (= ProcessEdgesWork::process_slots)
        for i in 0..packet.len() {
            process_and_scan_slot(packet[i], &mut new_slots);
        }

        // Flush new slots into work packets (= flush() → create_scan_work → new ProcessEdgesWork)
        for chunk in new_slots.chunks(packet_size) {
            work_queue.push_back(chunk.to_vec());
        }
        marked_count += new_slots.len() / N_REFS; // each newly marked obj produces N_REFS child slots
        new_slots.clear();
    }

    marked_count
}

/// BFS tracing with edge prefetching at distance D.
/// Prefetches the slot address (slot content cache line) D positions ahead.
fn trace_bfs_edge_prefetch<const D: usize, const USE_T0: bool>(
    roots: &[BenchSlot],
    packet_size: usize,
) -> usize {
    let mut work_queue: VecDeque<Vec<BenchSlot>> = VecDeque::new();
    for chunk in roots.chunks(packet_size) {
        work_queue.push_back(chunk.to_vec());
    }

    let mut marked_count = 0usize;
    let mut new_slots = Vec::with_capacity(packet_size * 2);

    while let Some(packet) = work_queue.pop_front() {
        let len = packet.len();
        for i in 0..len {
            if i + D < len {
                if USE_T0 {
                    prefetch_t0(packet[i + D]);
                } else {
                    prefetch_nta(packet[i + D]);
                }
            }
            process_and_scan_slot(packet[i], &mut new_slots);
        }

        for chunk in new_slots.chunks(packet_size) {
            work_queue.push_back(chunk.to_vec());
        }
        marked_count += new_slots.len() / N_REFS;
        new_slots.clear();
    }

    marked_count
}

/// BFS tracing with object prefetching at distance D.
/// Loads slot at i+D to get objref, then prefetches the object header.
fn trace_bfs_object_prefetch<const D: usize, const USE_T0: bool>(
    roots: &[BenchSlot],
    packet_size: usize,
) -> usize {
    let mut work_queue: VecDeque<Vec<BenchSlot>> = VecDeque::new();
    for chunk in roots.chunks(packet_size) {
        work_queue.push_back(chunk.to_vec());
    }

    let mut marked_count = 0usize;
    let mut new_slots = Vec::with_capacity(packet_size * 2);

    while let Some(packet) = work_queue.pop_front() {
        let len = packet.len();
        for i in 0..len {
            if i + D < len {
                let future_objref: Option<ObjectReference> = Slot::load(&packet[i + D]);
                if let Some(obj) = future_objref {
                    if USE_T0 {
                        prefetch_t0(obj.to_raw_address());
                    } else {
                        prefetch_nta(obj.to_raw_address());
                    }
                }
            }
            process_and_scan_slot(packet[i], &mut new_slots);
        }

        for chunk in new_slots.chunks(packet_size) {
            work_queue.push_back(chunk.to_vec());
        }
        marked_count += new_slots.len() / N_REFS;
        new_slots.clear();
    }

    marked_count
}

/// BFS tracing with combined edge + object prefetching.
fn trace_bfs_edge_and_object<const E: usize, const O: usize, const USE_T0: bool>(
    roots: &[BenchSlot],
    packet_size: usize,
) -> usize {
    let mut work_queue: VecDeque<Vec<BenchSlot>> = VecDeque::new();
    for chunk in roots.chunks(packet_size) {
        work_queue.push_back(chunk.to_vec());
    }

    let mut marked_count = 0usize;
    let mut new_slots = Vec::with_capacity(packet_size * 2);

    while let Some(packet) = work_queue.pop_front() {
        let len = packet.len();
        for i in 0..len {
            // Edge prefetch
            if i + E < len {
                if USE_T0 {
                    prefetch_t0(packet[i + E]);
                } else {
                    prefetch_nta(packet[i + E]);
                }
            }
            // Object prefetch
            if i + O < len {
                let future_objref: Option<ObjectReference> = Slot::load(&packet[i + O]);
                if let Some(obj) = future_objref {
                    if USE_T0 {
                        prefetch_t0(obj.to_raw_address());
                    } else {
                        prefetch_nta(obj.to_raw_address());
                    }
                }
            }
            process_and_scan_slot(packet[i], &mut new_slots);
        }

        for chunk in new_slots.chunks(packet_size) {
            work_queue.push_back(chunk.to_vec());
        }
        marked_count += new_slots.len() / N_REFS;
        new_slots.clear();
    }

    marked_count
}

// ───────────────────────────────────────────────────────────────────────────
// Build root slots
// ───────────────────────────────────────────────────────────────────────────

/// Build initial root slots: reference fields of the first `n_roots` objects.
/// This seeds the BFS traversal, like scan_roots in real GC.
fn build_root_slots(objects: &[ObjectReference], n_roots: usize) -> Vec<BenchSlot> {
    let mut roots = Vec::with_capacity(n_roots * N_REFS);
    for i in 0..n_roots.min(objects.len()) {
        for r in 0..N_REFS {
            roots.push(ref_field_addr(objects[i], r));
        }
    }
    roots
}

// ───────────────────────────────────────────────────────────────────────────
// Correctness
// ───────────────────────────────────────────────────────────────────────────

fn verify_bfs(objects: &[ObjectReference], roots: &[BenchSlot]) {
    clear_marks(objects);
    let baseline = trace_bfs_baseline(roots, PACKET_SIZE);

    clear_marks(objects);
    let edge16 = trace_bfs_edge_prefetch::<16, false>(roots, PACKET_SIZE);

    clear_marks(objects);
    let obj16 = trace_bfs_object_prefetch::<16, false>(roots, PACKET_SIZE);

    clear_marks(objects);
    let combined = trace_bfs_edge_and_object::<32, 16, false>(roots, PACKET_SIZE);

    assert_eq!(
        baseline, edge16,
        "edge_pf_16 marked {} objects, baseline marked {}",
        edge16, baseline
    );
    assert_eq!(
        baseline, obj16,
        "object_pf_16 marked {} objects, baseline marked {}",
        obj16, baseline
    );
    assert_eq!(
        baseline, combined,
        "combined marked {} objects, baseline marked {}",
        combined, baseline
    );

    eprintln!(
        "[prefetch_tracing] BFS correctness verified: all strategies mark {} objects from {} root slots",
        baseline,
        roots.len()
    );
    clear_marks(objects);
}

// ───────────────────────────────────────────────────────────────────────────
// Benchmark entry point
// ───────────────────────────────────────────────────────────────────────────

pub fn bench(c: &mut Criterion) {
    std::env::set_var("MMTK_PLAN", "MarkSweep");

    write_mockvm(|mock| {
        *mock = MockVM {
            is_collection_enabled: MockMethod::new_fixed(Box::new(|_| true)),
            get_object_size: MockMethod::new_fixed(Box::new(|_| OBJ_SIZE)),
            ..MockVM::default()
        };
    });

    // 4M objects × 256 bytes ≈ 1 GB — well beyond 32 MB per-CCD L3.
    let n_objects: usize = 4 << 20;
    let heap_size = n_objects * OBJ_SIZE * 2;

    eprintln!(
        "[prefetch_tracing] Allocating {} objects ({} MB, {} bytes/obj, {} MB heap)...",
        n_objects,
        n_objects * OBJ_SIZE / (1 << 20),
        OBJ_SIZE,
        heap_size / (1 << 20)
    );

    let mut fixture = MutatorFixture::create_with_heapsize(heap_size);
    let objects = allocate_objects(&mut fixture, n_objects);

    let mut klass_storage = Vec::new();
    build_random_dag_with_klass(&objects, &mut klass_storage);

    // Use 256 root objects (1024 root slots).
    // With 4M objects and random connectivity, BFS from 256 roots will
    // reach nearly all objects, producing thousands of work packets.
    let n_roots = 256;
    let root_slots = build_root_slots(&objects, n_roots);

    eprintln!(
        "[prefetch_tracing] {} root slots from {} root objects, {} total objects, packet_size={}",
        root_slots.len(),
        n_roots,
        objects.len(),
        PACKET_SIZE,
    );

    verify_bfs(&objects, &root_slots);

    let measurement_time = Duration::from_secs(15);

    // ── Group 1: Object prefetch distance sweep (NTA) ───────────────────
    // Object prefetching is the primary contributor to performance gains.
    {
        let mut group = c.benchmark_group("object_prefetch_nta");
        group.measurement_time(measurement_time);

        group.bench_function("baseline", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_baseline(
                    black_box(&root_slots),
                    PACKET_SIZE,
                ))
            });
        });
        group.bench_function("distance_4", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_object_prefetch::<4, false>(
                    black_box(&root_slots),
                    PACKET_SIZE,
                ))
            });
        });
        group.bench_function("distance_8", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_object_prefetch::<8, false>(
                    black_box(&root_slots),
                    PACKET_SIZE,
                ))
            });
        });
        group.bench_function("distance_16", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_object_prefetch::<16, false>(
                    black_box(&root_slots),
                    PACKET_SIZE,
                ))
            });
        });
        group.bench_function("distance_32", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_object_prefetch::<32, false>(
                    black_box(&root_slots),
                    PACKET_SIZE,
                ))
            });
        });
        group.finish();
    }

    // ── Group 2: Edge prefetch distance sweep (NTA) ─────────────────────
    {
        let mut group = c.benchmark_group("edge_prefetch_nta");
        group.measurement_time(measurement_time);

        group.bench_function("baseline", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_baseline(
                    black_box(&root_slots),
                    PACKET_SIZE,
                ))
            });
        });
        group.bench_function("distance_4", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_edge_prefetch::<4, false>(
                    black_box(&root_slots),
                    PACKET_SIZE,
                ))
            });
        });
        group.bench_function("distance_8", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_edge_prefetch::<8, false>(
                    black_box(&root_slots),
                    PACKET_SIZE,
                ))
            });
        });
        group.bench_function("distance_16", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_edge_prefetch::<16, false>(
                    black_box(&root_slots),
                    PACKET_SIZE,
                ))
            });
        });
        group.bench_function("distance_32", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_edge_prefetch::<32, false>(
                    black_box(&root_slots),
                    PACKET_SIZE,
                ))
            });
        });
        group.finish();
    }

    // ── Group 3: Combined edge+object (sweep around Huang's E=32, O=16) ─
    {
        let mut group = c.benchmark_group("combined_prefetch_nta");
        group.measurement_time(measurement_time);

        group.bench_function("baseline", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_baseline(black_box(&root_slots), PACKET_SIZE))
            });
        });
        group.bench_function("e32_o4", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_edge_and_object::<32, 4, false>(
                    black_box(&root_slots), PACKET_SIZE,
                ))
            });
        });
        group.bench_function("e32_o8", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_edge_and_object::<32, 8, false>(
                    black_box(&root_slots), PACKET_SIZE,
                ))
            });
        });
        group.bench_function("e32_o16", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_edge_and_object::<32, 16, false>(
                    black_box(&root_slots), PACKET_SIZE,
                ))
            });
        });
        group.bench_function("e32_o32", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_edge_and_object::<32, 32, false>(
                    black_box(&root_slots), PACKET_SIZE,
                ))
            });
        });
        group.bench_function("e4_o16", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_edge_and_object::<4, 16, false>(
                    black_box(&root_slots), PACKET_SIZE,
                ))
            });
        });
        group.bench_function("e8_o16", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_edge_and_object::<8, 16, false>(
                    black_box(&root_slots), PACKET_SIZE,
                ))
            });
        });
        group.bench_function("e16_o16", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_edge_and_object::<16, 16, false>(
                    black_box(&root_slots), PACKET_SIZE,
                ))
            });
        });
        group.finish();
    }

    // ── Group 4: NTA vs T0 locality hint ────────────────────────────────
    {
        let mut group = c.benchmark_group("nta_vs_t0");
        group.measurement_time(measurement_time);

        group.bench_function("baseline", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_baseline(black_box(&root_slots), PACKET_SIZE))
            });
        });
        group.bench_function("e32_o16_nta", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_edge_and_object::<32, 16, false>(
                    black_box(&root_slots), PACKET_SIZE,
                ))
            });
        });
        group.bench_function("e32_o16_t0", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_edge_and_object::<32, 16, true>(
                    black_box(&root_slots), PACKET_SIZE,
                ))
            });
        });
        group.bench_function("obj16_nta", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_object_prefetch::<16, false>(
                    black_box(&root_slots), PACKET_SIZE,
                ))
            });
        });
        group.bench_function("obj16_t0", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_object_prefetch::<16, true>(
                    black_box(&root_slots), PACKET_SIZE,
                ))
            });
        });
        group.finish();
    }

    // ── Group 5: Packet size effect ─────────────────────────────────────
    // Atkinson noted small work packets limit prefetch effectiveness.
    {
        let mut group = c.benchmark_group("packet_size_effect");
        group.measurement_time(measurement_time);

        for &pkt_size in &[64usize, 256, 1024, 4096] {
            group.bench_function(format!("baseline_pkt_{}", pkt_size), |b| {
                b.iter(|| {
                    clear_marks(&objects);
                    black_box(trace_bfs_baseline(black_box(&root_slots), pkt_size))
                });
            });

            group.bench_function(format!("pf_e32_o16_pkt_{}", pkt_size), |b| {
                b.iter(|| {
                    clear_marks(&objects);
                    black_box(trace_bfs_edge_and_object::<32, 16, false>(
                        black_box(&root_slots), pkt_size,
                    ))
                });
            });
        }
        group.finish();
    }
}
