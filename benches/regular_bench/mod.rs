pub use criterion::Criterion;

mod bulk_meta;
mod immix_bench;
mod object_forwarding;
mod tracing_sim;

pub fn bench(c: &mut Criterion) {
    bulk_meta::bench(c);
    immix_bench::bench(c);
    tracing_sim::bench(c);
    object_forwarding::bench(c);
}
