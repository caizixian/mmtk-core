pub use criterion::Criterion;

mod bulk_meta;
mod tracing_sim;

pub fn bench(c: &mut Criterion) {
    bulk_meta::bench(c);
    tracing_sim::bench(c);
}
