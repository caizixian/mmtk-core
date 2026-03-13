pub use criterion::Criterion;

mod bulk_meta;
mod immix_bench;

pub fn bench(c: &mut Criterion) {
    bulk_meta::bench(c);
    immix_bench::bench(c);
}
