use criterion::Criterion;

pub mod alloc;
pub mod amac_tracing;
pub mod forwarding;
pub mod immix;
pub mod internal_pointer;
pub mod line_mark_scan;
pub mod mmapper;
pub mod prefetch_tracing;
pub mod sft;
pub mod simd_tracing;
pub mod tracing;

// As we can only initialize one MMTk instance, we have to run each benchmark in a separate process.
// So we only register one benchmark to criterion ('bench_main'), and based on the env var MMTK_BENCH,
// we pick the right benchmark to run.

// The benchmark can be executed with the following command. The feature `mock_test` is required, as the tests use MockVM.
// MMTK_BENCH=alloc cargo bench --features mock_test
// MMTK_BENCH=sft   cargo bench --features mock_test
// MMTK_BENCH=immix cargo bench --features mock_test
// MMTK_BENCH=forwarding cargo bench --features mock_test
// MMTK_BENCH=tracing MMTK_PLAN=MarkSweep cargo bench --features 'mock_test immortal_as_nonmoving'
// MMTK_BENCH=simd_tracing MMTK_PLAN=MarkSweep cargo bench --features 'mock_test immortal_as_nonmoving'
// MMTK_BENCH=prefetch_tracing MMTK_PLAN=MarkSweep cargo bench --features 'mock_test immortal_as_nonmoving'

// [Yi] I am not sure if these benchmarks are helpful any more after the MockVM refactoring. MockVM is really slow, as it
// is accessed with a lock, and it dispatches every call to function pointers in a struct. These tests may use MockVM,
// so they become slower as well. And the slowdown
// from MockVM may hide the actual performance difference when we change the functions that are benchmarked.
// We may want to improve the MockVM implementation so we can skip dispatching for benchmarking, or introduce another MockVM
// implementation for benchmarking.
// However, I will just keep these benchmarks here. If we find it not useful, and we do not plan to improve MockVM, we can delete
// them.

pub fn bench(c: &mut Criterion) {
    match std::env::var("MMTK_BENCH") {
        Ok(bench) => match bench.as_str() {
            "alloc" => alloc::bench(c),
            "amac_tracing" => amac_tracing::bench(c),
            "forwarding" => forwarding::bench(c),
            "immix" => immix::bench(c),
            "internal_pointer" => internal_pointer::bench(c),
            "line_mark_scan" => line_mark_scan::bench(c),
            "mmapper" => mmapper::bench(c),
            "prefetch_tracing" => prefetch_tracing::bench(c),
            "sft" => sft::bench(c),
            "simd_tracing" => simd_tracing::bench(c),
            "tracing" => tracing::bench(c),
            _ => panic!("Unknown benchmark {:?}", bench),
        },
        Err(_) => panic!("Need to name a benchmark by the env var MMTK_BENCH"),
    }
}
