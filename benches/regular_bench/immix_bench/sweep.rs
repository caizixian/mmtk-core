//! Benchmark for Immix block sweep operation.
//!
//! Simulates the inner loop of `Block::sweep()` which counts marked lines,
//! holes, and builds the defrag histogram for a block. This is executed for
//! every allocated block during GC release, making it a significant time
//! consumer.
//!
//! The sweep loop iterates all lines in a block (typically 128 lines),
//! checking each line's mark state and counting contiguous holes.

use criterion::{criterion_group, Criterion};
use mmtk::util::test_private::{BLOCK_LINES, LINE_RESET_MARK_STATE};
use rand::{seq::IteratorRandom, SeedableRng};
use rand_chacha::ChaCha8Rng;

/// Get a deterministic seeded Rng.
fn get_rng() -> ChaCha8Rng {
    const SEED64: u64 = 0x8e3c6a1f5d2b9074;
    ChaCha8Rng::seed_from_u64(SEED64)
}

/// Result of sweeping a block.
#[derive(Debug, PartialEq)]
enum SweepResult {
    /// Block is completely free (no marked lines).
    Free,
    /// Block is fully occupied (all lines marked).
    Full,
    /// Block has some available lines. Contains (marked_lines, holes).
    Reusable { marked_lines: usize, holes: usize },
}

/// Simulate the sweep loop from `Block::sweep()` (block.rs:235-298).
///
/// This is the exact algorithm: iterate all lines, count marked_lines and holes,
/// then decide the block state.
#[inline(never)]
fn sweep_block(mark_data: &[u8], line_mark_state: u8) -> SweepResult {
    let mut marked_lines: usize = 0;
    let mut holes: usize = 0;
    let mut prev_line_is_marked = true;

    for &mark in mark_data.iter() {
        if mark == line_mark_state {
            marked_lines += 1;
            prev_line_is_marked = true;
        } else {
            if prev_line_is_marked {
                holes += 1;
            }
            prev_line_is_marked = false;
        }
    }

    if marked_lines == 0 {
        SweepResult::Free
    } else if marked_lines == mark_data.len() {
        SweepResult::Full
    } else {
        SweepResult::Reusable {
            marked_lines,
            holes,
        }
    }
}

/// Sweep a block and also populate a histogram of holes → marked lines.
/// This matches the production code's `mark_histogram[holes] += marked_lines`.
#[inline(never)]
fn sweep_block_with_histogram(
    mark_data: &[u8],
    line_mark_state: u8,
    histogram: &mut [usize],
) -> SweepResult {
    let result = sweep_block(mark_data, line_mark_state);
    if let SweepResult::Reusable {
        marked_lines,
        holes,
    } = &result
    {
        if *holes < histogram.len() {
            histogram[*holes] += *marked_lines;
        }
    }
    result
}

/// Create a line mark table with the given number of marked lines spread randomly.
fn make_mark_table(num_marked: usize, mark_state: u8) -> Vec<u8> {
    let mut data = vec![0u8; BLOCK_LINES];
    let mut rng = get_rng();
    let marked_indices: Vec<usize> = (0..BLOCK_LINES).choose_multiple(&mut rng, num_marked);
    for idx in marked_indices {
        data[idx] = mark_state;
    }
    data
}

/// Create a line mark table with clustered marked regions.
fn make_clustered_mark_table(occupancy: f64, mark_state: u8) -> Vec<u8> {
    let mut data = vec![0u8; BLOCK_LINES];
    let num_marked = (BLOCK_LINES as f64 * occupancy) as usize;
    let cluster_size = 8; // typical object spans a few lines
    let mut i = 0;
    let mut marked_so_far = 0;
    let mut marking = true;

    while i < BLOCK_LINES && marked_so_far < num_marked {
        let remaining = BLOCK_LINES - i;
        let this_cluster = cluster_size.min(remaining);

        if marking {
            let can_mark = (num_marked - marked_so_far).min(this_cluster);
            for j in i..i + can_mark {
                data[j] = mark_state;
            }
            marked_so_far += can_mark;
        }
        i += this_cluster;
        marking = !marking;
    }

    data
}

pub fn bench(c: &mut Criterion) {
    let mark_state = LINE_RESET_MARK_STATE;

    // The defrag histogram bins: one per possible hole count
    // In practice, max holes = BLOCK_LINES / 2 + 1
    let histogram_size = (BLOCK_LINES / 2) + 1;

    // Sweep fully marked block (best case — fast path, no holes)
    c.bench_function("immix_sweep_full", |b| {
        let data = vec![mark_state; BLOCK_LINES];
        b.iter(|| {
            let result = sweep_block(&data, mark_state);
            criterion::black_box(result);
        });
    });

    // Sweep fully free block (best case — immediate return)
    c.bench_function("immix_sweep_free", |b| {
        let data = vec![0u8; BLOCK_LINES];
        b.iter(|| {
            let result = sweep_block(&data, mark_state);
            criterion::black_box(result);
        });
    });

    // Sweep 50% occupied block (random fragmentation)
    c.bench_function("immix_sweep_50pct", |b| {
        let data = make_mark_table(BLOCK_LINES / 2, mark_state);
        b.iter(|| {
            let result = sweep_block(&data, mark_state);
            criterion::black_box(result);
        });
    });

    // Sweep 50% occupied with histogram (includes defrag bookkeeping)
    c.bench_function("immix_sweep_50pct_histogram", |b| {
        let data = make_mark_table(BLOCK_LINES / 2, mark_state);
        let mut histogram = vec![0usize; histogram_size];
        b.iter(|| {
            histogram.iter_mut().for_each(|x| *x = 0);
            let result = sweep_block_with_histogram(&data, mark_state, &mut histogram);
            criterion::black_box(result);
        });
    });

    // Sweep clustered 60% occupancy (realistic post-GC pattern)
    c.bench_function("immix_sweep_clustered_60pct", |b| {
        let data = make_clustered_mark_table(0.6, mark_state);
        b.iter(|| {
            let result = sweep_block(&data, mark_state);
            criterion::black_box(result);
        });
    });

    // Sweep many blocks in sequence (simulates per-chunk sweep work packet)
    c.bench_function("immix_sweep_chunk_128blocks", |b| {
        // A chunk contains multiple blocks. Simulate sweeping 128 blocks in sequence.
        let tables: Vec<Vec<u8>> = (0..128)
            .map(|i| {
                if i % 4 == 0 {
                    vec![0u8; BLOCK_LINES] // 25% free blocks
                } else {
                    make_mark_table(BLOCK_LINES * 3 / 4, mark_state) // 75% occupied
                }
            })
            .collect();
        let mut histogram = vec![0usize; histogram_size];

        b.iter(|| {
            histogram.iter_mut().for_each(|x| *x = 0);
            let mut freed = 0;
            for table in &tables {
                let result = sweep_block_with_histogram(table, mark_state, &mut histogram);
                if result == SweepResult::Free {
                    freed += 1;
                }
            }
            criterion::black_box(freed);
        });
    });
}
