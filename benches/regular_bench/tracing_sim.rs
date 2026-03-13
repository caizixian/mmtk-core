//! Benchmarks simulating tracing hot loops.
//!
//! These benchmarks extract the algorithmic patterns from the tracing subsystem
//! and run them on synthetic data, without requiring a VM or full MMTk state.
//! They measure the pure cost of the inner loops that dominate GC tracing time.

use criterion::Criterion;
use mmtk::util::test_private::WORK_BUFFER_CAPACITY;
use std::sync::atomic::{AtomicU8, Ordering};

/// Simulate the CAS marking pattern from `ImmixSpace::attempt_mark()`.
///
/// The actual code (immixspace.rs:781-807) does:
///   1. Atomic load of the mark bit
///   2. If already marked, return false
///   3. Atomic CAS to set the mark bit
///   4. Loop on CAS failure
///
/// This benchmark measures the uncontended single-thread cost of this pattern,
/// which dominates trace_object_without_moving. We use an array of AtomicU8
/// to simulate per-object mark bytes.
mod mark_cas {
    use super::*;

    const NUM_OBJECTS: usize = 4096;
    const UNMARKED: u8 = 0;
    const MARKED: u8 = 1;

    /// Allocate an array of atomic mark bytes, all initially unmarked.
    fn make_mark_bytes() -> Vec<AtomicU8> {
        (0..NUM_OBJECTS)
            .map(|_| AtomicU8::new(UNMARKED))
            .collect()
    }

    /// Reset all mark bytes to unmarked.
    fn reset_marks(marks: &[AtomicU8]) {
        for m in marks {
            m.store(UNMARKED, Ordering::Relaxed);
        }
    }

    /// The CAS marking pattern: returns true if we newly marked the object.
    #[inline(always)]
    fn attempt_mark(mark_byte: &AtomicU8, mark_state: u8) -> bool {
        loop {
            let old = mark_byte.load(Ordering::SeqCst);
            if old == mark_state {
                return false; // already marked
            }
            if mark_byte
                .compare_exchange(old, mark_state, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return true; // newly marked
            }
        }
    }

    pub fn bench(c: &mut Criterion) {
        // Benchmark 1: Mark all objects (all unmarked → marked), simulating first visit
        c.bench_function("trace_mark_cas_all_new", |b| {
            let marks = make_mark_bytes();
            b.iter(|| {
                reset_marks(&marks);
                let mut count = 0u32;
                for m in marks.iter() {
                    if attempt_mark(m, MARKED) {
                        count += 1;
                    }
                }
                assert_eq!(count as usize, NUM_OBJECTS);
                criterion::black_box(count);
            });
        });

        // Benchmark 2: Mark objects that are all already marked (simulating revisits)
        c.bench_function("trace_mark_cas_all_marked", |b| {
            let marks = make_mark_bytes();
            // Pre-mark all
            for m in marks.iter() {
                m.store(MARKED, Ordering::Relaxed);
            }
            b.iter(|| {
                let mut count = 0u32;
                for m in marks.iter() {
                    if attempt_mark(m, MARKED) {
                        count += 1;
                    }
                }
                assert_eq!(count, 0);
                criterion::black_box(count);
            });
        });

        // Benchmark 3: Mixed - 50% already marked (simulating shared heap graph)
        c.bench_function("trace_mark_cas_50pct_new", |b| {
            let marks = make_mark_bytes();
            b.iter(|| {
                // Pre-mark even indices
                for (i, m) in marks.iter().enumerate() {
                    m.store(if i % 2 == 0 { MARKED } else { UNMARKED }, Ordering::Relaxed);
                }
                let mut count = 0u32;
                for m in marks.iter() {
                    if attempt_mark(m, MARKED) {
                        count += 1;
                    }
                }
                criterion::black_box(count);
            });
        });
    }
}

/// Simulate the edge processing loop from `ProcessEdgesWork::process_slots()`.
///
/// The actual code (gc_work.rs:661-666) does:
///   for i in 0..self.slots.len() {
///       self.process_slot(self.slots[i])
///   }
/// where process_slot loads a reference, traces it (mark CAS + enqueue),
/// and optionally stores back a new reference.
///
/// We simulate this with an array of "slot" pointers (addresses) that need to
/// be loaded, marked, and enqueued. This captures the memory access pattern and
/// enqueue overhead without needing real objects.
mod edge_processing {
    use super::*;

    /// Simulate a slot as a raw address pointing to an object.
    /// In production, this is a VM::VMSlot that loads an ObjectReference.
    #[derive(Clone, Copy)]
    struct SimSlot {
        /// Index into the mark array (simulated object reference)
        object_index: usize,
    }

    /// Simulate the process_slots loop: load slot → mark object CAS → enqueue.
    #[inline(never)]
    fn process_edges(
        slots: &[SimSlot],
        mark_bytes: &[AtomicU8],
        queue: &mut Vec<usize>,
        mark_state: u8,
    ) {
        for slot in slots {
            let obj_idx = slot.object_index;
            // Simulate trace_object: CAS mark
            let old = mark_bytes[obj_idx].load(Ordering::SeqCst);
            if old != mark_state {
                if mark_bytes[obj_idx]
                    .compare_exchange(old, mark_state, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    // Newly marked → enqueue
                    queue.push(obj_idx);
                }
            }
        }
    }

    pub fn bench(c: &mut Criterion) {
        let heap_size = 16384; // simulated objects
        let batch_size = WORK_BUFFER_CAPACITY; // typical ProcessEdgesWork packet size

        // Create simulated slots pointing to sequential objects
        let slots: Vec<SimSlot> = (0..batch_size)
            .map(|i| SimSlot {
                object_index: i % heap_size,
            })
            .collect();

        let mark_bytes: Vec<AtomicU8> =
            (0..heap_size).map(|_| AtomicU8::new(0)).collect();

        // Benchmark: Process a full batch of edges (all new objects)
        c.bench_function("trace_process_edges_4096", |b| {
            let mut queue = Vec::with_capacity(batch_size);
            b.iter(|| {
                // Reset marks
                for m in mark_bytes.iter() {
                    m.store(0, Ordering::Relaxed);
                }
                queue.clear();
                process_edges(&slots, &mark_bytes, &mut queue, 1);
                criterion::black_box(queue.len());
            });
        });

        // Benchmark: Process a batch where all objects are already marked (revisits)
        c.bench_function("trace_process_edges_all_marked", |b| {
            for m in mark_bytes.iter() {
                m.store(1, Ordering::Relaxed);
            }
            let mut queue = Vec::with_capacity(batch_size);
            b.iter(|| {
                queue.clear();
                process_edges(&slots, &mark_bytes, &mut queue, 1);
                criterion::black_box(queue.len());
            });
        });
    }
}

/// Benchmark VectorObjectQueue throughput (enqueue + take cycle).
///
/// VectorObjectQueue (VectorQueue<ObjectReference>) is the primary buffer
/// for discovered objects during tracing. It reserves CAPACITY (4096) entries
/// and drains via `take()` which does `std::mem::take(&mut buffer)`.
///
/// Since VectorObjectQueue uses ObjectReference which requires non-null values,
/// we benchmark VectorQueue<usize> which has the same memory layout and behavior.
mod object_queue {
    use super::*;

    pub fn bench(c: &mut Criterion) {
        // Benchmark: Fill queue to capacity, then take all
        c.bench_function("trace_queue_fill_take_4096", |b| {
            let mut queue: Vec<usize> = Vec::new();
            b.iter(|| {
                if queue.is_empty() {
                    queue.reserve(WORK_BUFFER_CAPACITY);
                }
                for i in 0..WORK_BUFFER_CAPACITY {
                    queue.push(i);
                }
                let taken = std::mem::take(&mut queue);
                criterion::black_box(taken.len());
            });
        });

        // Benchmark: Repeated push-take cycles (simulates multiple flushes)
        c.bench_function("trace_queue_multi_flush", |b| {
            b.iter(|| {
                let mut queue: Vec<usize> = Vec::new();
                let mut total = 0usize;
                // Simulate 10 flush cycles of varying sizes
                for cycle in 0..10 {
                    let size = (cycle + 1) * (WORK_BUFFER_CAPACITY / 10);
                    if queue.is_empty() {
                        queue.reserve(WORK_BUFFER_CAPACITY);
                    }
                    for i in 0..size {
                        queue.push(i);
                    }
                    let taken = std::mem::take(&mut queue);
                    total += taken.len();
                }
                criterion::black_box(total);
            });
        });
    }
}

pub fn bench(c: &mut Criterion) {
    mark_cas::bench(c);
    edge_processing::bench(c);
    object_queue::bench(c);
}
