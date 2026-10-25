//! Benchmarks for side metadata address translation.

use criterion::Criterion;
use mmtk::util::{
    constants::BYTES_IN_PAGE,
    constants::LOG_BYTES_IN_WORD,
    conversions::raw_align_up,
    metadata::side_metadata::SideMetadataSpec,
    os::{MmapAnnotation, MmapProtection, MmapStrategy, OSMemory, OS},
    test_private::address_to_meta_address,
    Address,
};
use std::hint::black_box;

// 1-bit side metadata per word, matching common side-metadata access patterns.
const BENCH_SPEC: SideMetadataSpec = SideMetadataSpec {
    name: "bench",
    is_global: true,
    offset: 0,
    log_num_of_bits: 0,
    log_bytes_in_region: LOG_BYTES_IN_WORD as usize,
};

const NUM_ADDRS: usize = 4096;
const DATA_STRIDE: usize = 64;
// Non-zero base ensures `address_to_meta_address` maps above page zero (`0x0040_0000` on 64-bit)
// even when standalone benchmarks run without initializing `SIDE_METADATA_BASE_ADDRESS`.
const BENCH_DATA_BASE: usize = 0x1000_0000;

fn prepare_data_addrs() -> Vec<Address> {
    (0..NUM_ADDRS)
        .map(|i| Address::ZERO + (BENCH_DATA_BASE + i * DATA_STRIDE))
        .collect()
}

pub fn bench(c: &mut Criterion) {
    c.bench_function("side_metadata_address_translation", |b| {
        let addrs = prepare_data_addrs();

        b.iter(|| {
            let mut checksum = 0usize;
            for data_addr in &addrs {
                let meta_addr = address_to_meta_address(&BENCH_SPEC, *data_addr);
                checksum ^= meta_addr.as_usize();
            }
            black_box(checksum);
        });
    });

    c.bench_function("side_metadata_load", |b| {
        let addrs = prepare_data_addrs();

        // Pre-map metadata range so the benchmark measures translation + load.
        let mut meta_min = Address::MAX;
        let mut meta_max = Address::ZERO;
        for data_addr in &addrs {
            let meta_addr = address_to_meta_address(&BENCH_SPEC, *data_addr);
            if meta_addr < meta_min {
                meta_min = meta_addr;
            }
            if meta_addr > meta_max {
                meta_max = meta_addr;
            }
        }
        let map_start = meta_min.align_down(BYTES_IN_PAGE);
        let map_size = raw_align_up((meta_max + 1usize) - map_start, BYTES_IN_PAGE);

        OS::dzmmap(
            map_start,
            map_size,
            MmapStrategy::default()
                .prot(MmapProtection::ReadWrite)
                .replace(true)
                .reserve(true),
            &MmapAnnotation::Misc {
                name: "bench-side-metadata-load",
            },
        )
        .unwrap_or_else(|e| panic!("failed to map side metadata for benchmark: {e}"));

        // Touch mapped metadata so load benchmark won't include first-touch page faults.
        mmtk::util::memory::set(map_start, 0x55, map_size);

        b.iter(|| {
            let mut checksum = 0usize;
            for data_addr in &addrs {
                // SAFETY: Every `data_addr` in `addrs` maps to a metadata byte inside
                // `[map_start, map_start + map_size)`, which was mapped read-write via `OS::dzmmap`
                // and initialized via `memory::set` above (`BENCH_SPEC.log_num_of_bits == 0` uses
                // 1-byte `u8` loads), and this single-threaded benchmark performs no concurrent or
                // atomic accesses during `b.iter`.
                let val = unsafe { BENCH_SPEC.load::<u8>(*data_addr) };
                checksum ^= val as usize;
            }
            black_box(checksum);
        });
    });
}
