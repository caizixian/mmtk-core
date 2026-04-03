//! Benchmarks for bulk zeroing and setting.

use criterion::Criterion;
use mmtk::util::{constants::LOG_BITS_IN_WORD, test_private, Address};

const LINE_BYTES: usize = 256usize; // Match an Immix line size.
const BLOCK_BYTES: usize = 32768usize; // Match an Immix block size.

// Asssume one-bit-per-word metadata (matching VO bits).
const LINE_META_BYTES: usize = LINE_BYTES >> LOG_BITS_IN_WORD;
const BLOCK_META_BYTES: usize = BLOCK_BYTES >> LOG_BITS_IN_WORD;

pub fn bench(c: &mut Criterion) {
    c.bench_function("bzero_bset_line", |b| {
        let mut data = vec![0u8; LINE_META_BYTES];
        let start = Address::from_mut_ptr(data.as_mut_ptr());
        let end = start + LINE_META_BYTES;

        b.iter(|| {
            test_private::set_meta_bits(start, 0, end, 0);
            test_private::zero_meta_bits(start, 0, end, 0);
        })
    });

    c.bench_function("bzero_bset_line_memset", |b| {
        let mut data = vec![0u8; LINE_META_BYTES];

        b.iter(|| {
            data.fill(0xff);
            data.fill(0x00);
        })
    });

    c.bench_function("bzero_bset_block", |b| {
        let mut data = vec![0u8; BLOCK_META_BYTES];
        let start = Address::from_mut_ptr(data.as_mut_ptr());
        let end = start + BLOCK_META_BYTES;

        b.iter(|| {
            test_private::set_meta_bits(start, 0, end, 0);
            test_private::zero_meta_bits(start, 0, end, 0);
        })
    });

    c.bench_function("bzero_bset_block_memset", |b| {
        let mut data = vec![0u8; BLOCK_META_BYTES];

        b.iter(|| {
            data.fill(0xff);
            data.fill(0x00);
        })
    });
}
