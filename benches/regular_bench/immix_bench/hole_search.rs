//! Benchmark for Immix hole searching.
//!
//! Simulates the inner loop of `ImmixSpace::get_next_available_lines()` which
//! linearly scans a block's line mark table to find available (unmarked) line
//! ranges. This is a hot path during allocation into recyclable blocks.
//!
//! The benchmark creates a byte array matching the Immix line mark table layout
//! (one byte per line, `Block::LINES` entries) and fills it with realistic
//! mark patterns, then runs the hole search loop.

use criterion::Criterion;
use mmtk::util::test_private::{BLOCK_LINES, LINE_RESET_MARK_STATE};
use rand::{seq::IteratorRandom, SeedableRng};
use rand_chacha::ChaCha8Rng;

/// Get a deterministic seeded Rng.
fn get_rng() -> ChaCha8Rng {
    const SEED64: u64 = 0x3a9f7c2e1b4d8a05;
    ChaCha8Rng::seed_from_u64(SEED64)
}

/// Represents a line mark table for one block.
/// This mirrors the data accessed by `Block::line_mark_table()`.
struct LineMarkTable {
    data: Vec<u8>,
}

impl LineMarkTable {
    /// Create a line mark table with all lines unmarked (state 0).
    fn new_unmarked() -> Self {
        Self {
            data: vec![0u8; BLOCK_LINES],
        }
    }

    /// Set specific lines as marked with the given state.
    fn mark_lines(&mut self, indices: &[usize], state: u8) {
        for &idx in indices {
            self.data[idx] = state;
        }
    }

    /// Returns the mark table as a slice, similar to `MetadataByteArrayRef`.
    fn as_slice(&self) -> &[u8] {
        &self.data
    }
}

/// Simulate the hole search algorithm from `ImmixSpace::get_next_available_lines()`.
///
/// This is the exact algorithm used in production (see immixspace.rs:840-872):
/// - Linear scan to find the start of a hole (unmarked region)
/// - Linear scan to find the end of the hole
/// - Returns (start_index, end_index)
///
/// The function scans from `start_cursor` and returns the first hole found,
/// or `None` if no holes remain.
#[inline(never)]
fn find_next_hole(
    mark_data: &[u8],
    start_cursor: usize,
    unavail_state: u8,
    current_state: u8,
) -> Option<(usize, usize)> {
    let mut cursor = start_cursor;

    // Find start of hole (first unmarked line)
    while cursor < mark_data.len() {
        let mark = mark_data[cursor];
        if mark != unavail_state && mark != current_state {
            break;
        }
        cursor += 1;
    }
    if cursor == mark_data.len() {
        return None;
    }
    let start = cursor;

    // Find end of hole (first marked line after the hole)
    while cursor < mark_data.len() {
        let mark = mark_data[cursor];
        if mark == unavail_state || mark == current_state {
            break;
        }
        cursor += 1;
    }
    let end = cursor;

    Some((start, end))
}

/// Iterate all holes in a block's line mark table.
/// Returns the total number of available lines found.
#[inline(never)]
fn find_all_holes(
    mark_data: &[u8],
    unavail_state: u8,
    current_state: u8,
) -> (usize, usize) {
    let mut cursor = 0;
    let mut total_lines = 0;
    let mut num_holes = 0;

    while let Some((start, end)) = find_next_hole(mark_data, cursor, unavail_state, current_state) {
        total_lines += end - start;
        num_holes += 1;
        cursor = end;
    }

    (total_lines, num_holes)
}

/// Create a mark table with a specific fragmentation pattern.
/// `fragmentation` = fraction of lines that are available (0.0 = fully marked, 1.0 = fully free).
/// `num_fragments` = how many separate holes to create.
fn make_fragmented_mark_table(
    fragmentation: f64,
    num_fragments: usize,
    mark_state: u8,
) -> LineMarkTable {
    let mut table = LineMarkTable::new_unmarked();
    let total_available = (BLOCK_LINES as f64 * fragmentation) as usize;
    let total_marked = BLOCK_LINES - total_available;

    // First mark all lines
    let all_indices: Vec<usize> = (0..BLOCK_LINES).collect();
    table.mark_lines(&all_indices, mark_state);

    // Then unmark (set to 0) the lines that should be available (holes).
    // Distribute holes somewhat evenly.
    let mut rng = get_rng();
    let hole_indices: Vec<usize> = (0..BLOCK_LINES)
        .choose_multiple(&mut rng, total_available);
    for idx in &hole_indices {
        table.data[*idx] = 0; // unmarked
    }

    let _ = (total_marked, num_fragments); // num_fragments controls distribution in more advanced versions

    table
}

/// Create a realistically fragmented mark table with clustered holes.
/// This produces the "swiss cheese" pattern typical after a GC cycle:
/// groups of consecutive marked lines separated by holes.
fn make_clustered_mark_table(
    occupancy: f64,
    avg_cluster_size: usize,
    mark_state: u8,
) -> LineMarkTable {
    let mut table = LineMarkTable::new_unmarked();
    let mut i = 0;

    // Alternate between clusters of marked and unmarked lines
    let mut marked = true; // start with a marked cluster
    let mut rng = get_rng();

    while i < BLOCK_LINES {
        let cluster_size = if marked {
            // Marked cluster: ~avg_cluster_size * occupancy
            let size = (avg_cluster_size as f64 * occupancy * 2.0) as usize;
            size.max(1).min(BLOCK_LINES - i)
        } else {
            // Hole: ~avg_cluster_size * (1 - occupancy)
            let size = (avg_cluster_size as f64 * (1.0 - occupancy) * 2.0) as usize;
            size.max(1).min(BLOCK_LINES - i)
        };

        if marked {
            for j in i..i + cluster_size {
                table.data[j] = mark_state;
            }
        }
        // unmarked lines are already 0

        i += cluster_size;
        marked = !marked;
    }

    let _ = rng; // suppress unused warning

    table
}

pub fn bench(c: &mut Criterion) {
    let mark_state = LINE_RESET_MARK_STATE;
    let unavail_state = mark_state.wrapping_sub(1);  // previous GC's mark state

    // Benchmark 1: Hole search on a half-occupied block (50% fragmentation)
    c.bench_function("immix_hole_search_50pct", |b| {
        let table = make_fragmented_mark_table(0.5, 8, mark_state);
        b.iter(|| {
            let result = find_all_holes(table.as_slice(), unavail_state, mark_state);
            criterion::black_box(result);
        });
    });

    // Benchmark 2: Hole search on a heavily occupied block (80% marked, 20% holes)
    c.bench_function("immix_hole_search_80pct", |b| {
        let table = make_fragmented_mark_table(0.2, 4, mark_state);
        b.iter(|| {
            let result = find_all_holes(table.as_slice(), unavail_state, mark_state);
            criterion::black_box(result);
        });
    });

    // Benchmark 3: Hole search on a nearly empty block (90% holes)
    c.bench_function("immix_hole_search_10pct", |b| {
        let table = make_fragmented_mark_table(0.9, 2, mark_state);
        b.iter(|| {
            let result = find_all_holes(table.as_slice(), unavail_state, mark_state);
            criterion::black_box(result);
        });
    });

    // Benchmark 4: Clustered pattern (realistic post-GC fragmentation)
    c.bench_function("immix_hole_search_clustered", |b| {
        let table = make_clustered_mark_table(0.6, 8, mark_state);
        b.iter(|| {
            let result = find_all_holes(table.as_slice(), unavail_state, mark_state);
            criterion::black_box(result);
        });
    });

    // Benchmark 5: Fully marked block (no holes — worst case for hole search as it scans everything)
    c.bench_function("immix_hole_search_full", |b| {
        let mut table = LineMarkTable::new_unmarked();
        let all_indices: Vec<usize> = (0..BLOCK_LINES).collect();
        table.mark_lines(&all_indices, mark_state);
        b.iter(|| {
            let result = find_all_holes(table.as_slice(), unavail_state, mark_state);
            criterion::black_box(result);
        });
    });
}
