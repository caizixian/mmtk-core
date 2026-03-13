//! Benchmarks for Immix block/line operations.
//!
//! These benchmarks simulate the algorithmic kernels of Immix space management
//! without requiring a full VM or MMTk runtime. They allocate raw memory and
//! populate it with realistic line mark patterns, then run the same inner loops
//! used in production.

pub mod hole_search;
pub mod sweep;

pub use criterion::Criterion;

pub fn bench(c: &mut Criterion) {
    hole_search::bench(c);
    sweep::bench(c);
}
