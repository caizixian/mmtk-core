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

fn run_libc_memset_pair(buf: &mut [u8]) {
    let ptr = buf.as_mut_ptr().cast::<c_void>();
    let len = buf.len();
    // SAFETY: `ptr` and `len` come directly from exclusive slice `buf: &mut [u8]`, which is valid,
    // aligned, and writable across all `len` bytes for both `memset` calls.
    unsafe {
        libc::memset(ptr, 0xff, len);
        libc::memset(ptr, 0x00, len);
    }
}

pub fn bench(c: &mut Criterion) {
    c.bench_function("bzero_bset_line", |b| {
        let mut buf = Box::new(AlignedLineMeta([0u8; LINE_META_BYTES]));
        let start = Address::from_mut_ptr(buf.0.as_mut_ptr());
        let end = start + LINE_META_BYTES;

        b.iter(|| {
            // SAFETY: `[start, end)` covers all `LINE_META_BYTES` initialized, writable bytes of
            // `buf`, which is exclusively owned by this single-threaded benchmark closure with no
            // concurrent accesses or live Rust references during `b.iter`.
            unsafe {
                test_private::set_meta_bits(start, 0, end, 0);
                test_private::zero_meta_bits(start, 0, end, 0);
            }
        })
    });

    c.bench_function("bzero_bset_line_memset", |b| {
        let mut buf = Box::new(AlignedLineMeta([0u8; LINE_META_BYTES]));
        b.iter(|| run_libc_memset_pair(&mut buf.0))
    });

    c.bench_function("bzero_bset_block", |b| {
        let mut buf = Box::new(AlignedBlockMeta([0u8; BLOCK_META_BYTES]));
        let start = Address::from_mut_ptr(buf.0.as_mut_ptr());
        let end = start + BLOCK_META_BYTES;

        b.iter(|| {
            // SAFETY: `[start, end)` covers all `BLOCK_META_BYTES` initialized, writable bytes of
            // `buf`, which is exclusively owned by this single-threaded benchmark closure with no
            // concurrent accesses or live Rust references during `b.iter`.
            unsafe {
                test_private::set_meta_bits(start, 0, end, 0);
                test_private::zero_meta_bits(start, 0, end, 0);
            }
        })
    });

    c.bench_function("bzero_bset_block_memset", |b| {
        let mut buf = Box::new(AlignedBlockMeta([0u8; BLOCK_META_BYTES]));
        b.iter(|| run_libc_memset_pair(&mut buf.0))
    });
}
