//! Benchmarks for bulk zeroing and setting.

use std::os::raw::c_void;

use criterion::Criterion;
use mmtk::util::{constants::LOG_BITS_IN_WORD, test_private, Address};

const LINE_BYTES: usize = 256usize; // Match an Immix line size.
const BLOCK_BYTES: usize = 32768usize; // Match an Immix block size.

// Assume one-bit-per-word metadata (matching VO bits).
const LINE_META_BYTES: usize = LINE_BYTES >> LOG_BITS_IN_WORD;
const BLOCK_META_BYTES: usize = BLOCK_BYTES >> LOG_BITS_IN_WORD;

/// Aligned buffer for line-granularity metadata (`8 >= LINE_META_BYTES` on both 32-bit and 64-bit).
#[repr(C, align(8))]
struct AlignedLineMeta([u8; LINE_META_BYTES]);

/// Aligned buffer for block-granularity metadata (`1024 >= BLOCK_META_BYTES` on both 32-bit and 64-bit).
#[repr(C, align(1024))]
struct AlignedBlockMeta([u8; BLOCK_META_BYTES]);

pub fn bench(c: &mut Criterion) {
    c.bench_function("bzero_bset_line", |b| {
        let mut buf = Box::new(AlignedLineMeta([0u8; LINE_META_BYTES]));
        let start = Address::from_mut_ptr(buf.0.as_mut_ptr());
        let end = start + LINE_META_BYTES;

        b.iter(|| {
            test_private::set_meta_bits(start, 0, end, 0);
            test_private::zero_meta_bits(start, 0, end, 0);
        })
    });

    c.bench_function("bzero_bset_line_memset", |b| {
        let mut buf = Box::new(AlignedLineMeta([0u8; LINE_META_BYTES]));
        let start = Address::from_mut_ptr(buf.0.as_mut_ptr());
        let end = start + LINE_META_BYTES;

        b.iter(|| {
            let ptr = start.to_mut_ptr::<c_void>();
            let len = end - start;
            // SAFETY: `ptr` was derived via `buf.0.as_mut_ptr()` with mutable provenance over the
            // entire live, initialized `[u8; LINE_META_BYTES]` allocation owned by `buf` (`len ==
            // LINE_META_BYTES`), and `buf` is neither moved nor accessed directly during `b.iter`.
            unsafe {
                libc::memset(ptr, 0xff, len);
                libc::memset(ptr, 0x00, len);
            }
        })
    });

    c.bench_function("bzero_bset_block", |b| {
        let mut buf = Box::new(AlignedBlockMeta([0u8; BLOCK_META_BYTES]));
        let start = Address::from_mut_ptr(buf.0.as_mut_ptr());
        let end = start + BLOCK_META_BYTES;

        b.iter(|| {
            test_private::set_meta_bits(start, 0, end, 0);
            test_private::zero_meta_bits(start, 0, end, 0);
        })
    });

    c.bench_function("bzero_bset_block_memset", |b| {
        let mut buf = Box::new(AlignedBlockMeta([0u8; BLOCK_META_BYTES]));
        let start = Address::from_mut_ptr(buf.0.as_mut_ptr());
        let end = start + BLOCK_META_BYTES;

        b.iter(|| {
            let ptr = start.to_mut_ptr::<c_void>();
            let len = end - start;
            // SAFETY: `ptr` was derived via `buf.0.as_mut_ptr()` with mutable provenance over the
            // entire live, initialized `[u8; BLOCK_META_BYTES]` allocation owned by `buf` (`len ==
            // BLOCK_META_BYTES`), and `buf` is neither moved nor accessed directly during `b.iter`.
            unsafe {
                libc::memset(ptr, 0xff, len);
                libc::memset(ptr, 0x00, len);
            }
        })
    });
}
