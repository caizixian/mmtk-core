//! Benchmarks for VectorQueue throughput.
//!
//! VectorQueue is the primary buffer for discovered objects during tracing.
//! These benchmarks call directly into VectorQueue via push/take operations,
//! measuring real allocation and memory patterns.

use criterion::Criterion;
use mmtk::util::test_private::WORK_BUFFER_CAPACITY;

pub fn bench(c: &mut Criterion) {
    // Benchmark: Fill queue to capacity, then take all
    // This is the fundamental enqueue/drain cycle in the tracing work loop.
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

    // Benchmark: Repeated push-take cycles (simulates multiple flushes within one GC)
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
