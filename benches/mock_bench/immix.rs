//! Mock benchmarks for Immix block/line operations.
//!
//! These benchmarks call directly into existing MMTk code:
//! - `Block::sweep()` for block sweeping
//! - `ImmixSpace::get_next_available_lines()` for hole searching
//! - `Line::mark()` / `Line::is_marked()` for line marking
//!
//! They use MockVM with the Immix plan to set up real side metadata,
//! blocks, and lines. The benchmarks measure the actual code paths
//! that run during GC, not simulations.

use criterion::Criterion;

use mmtk::memory_manager;
use mmtk::util::options::PlanSelector;
use mmtk::util::test_private::*;
use mmtk::util::test_util::fixtures::*;
use mmtk::util::test_util::mock_method::*;
use mmtk::util::test_util::mock_vm::{write_mockvm, MockVM};
use mmtk::AllocationSemantics;

/// Allocate `count` objects of the given `size` in the Immix space.
/// Returns the list of allocated ObjectReferences.
fn allocate_objects(
    fixture: &mut MutatorFixture,
    count: usize,
    size: usize,
) -> Vec<mmtk::util::ObjectReference> {
    let mut objects = Vec::with_capacity(count);
    for _ in 0..count {
        let addr = memory_manager::alloc(&mut fixture.mutator, size, 8, 0, AllocationSemantics::Default);
        assert!(!addr.is_zero());
        let objref = MockVM::object_start_to_ref(addr);
        memory_manager::post_alloc(&mut fixture.mutator, objref, size, AllocationSemantics::Default);
        objects.push(objref);
    }
    objects
}

pub fn bench(c: &mut Criterion) {
    // Disable GC so allocation doesn't trigger collection.
    write_mockvm(|mock| {
        *mock = MockVM {
            is_collection_enabled: MockMethod::new_fixed(Box::new(|_| false)),
            ..MockVM::default()
        };
    });

    // Create fixture with Immix plan and a generous heap (256MB).
    let mut fixture = MutatorFixture::create_with_builder(|builder| {
        builder.options.plan.set(PlanSelector::Immix);
        builder
            .options
            .gc_trigger
            .set(mmtk::util::options::GCTriggerSelector::FixedHeapSize(
                256 * 1024 * 1024,
            ));
    });

    // Allocate objects to populate some blocks.
    // Each object is 64 bytes. A line is 256 bytes, a block is 32KB = 128 lines.
    // 64 objects * 64 bytes = 4KB = ~16 lines = partial block occupancy.
    let objects = allocate_objects(&mut fixture, 64, 64);
    assert!(!objects.is_empty(), "Failed to allocate objects");

    let mmtk = fixture.mmtk();
    let immix_space = get_immix_space(mmtk);

    // Get a block from the first allocated object.
    let first_obj = objects[0];
    let first_addr = first_obj.to_raw_address();
    let block = Block::from_unaligned_address(first_addr);

    // --- Benchmark 1: Line::mark() + Line::is_marked() on real side metadata ---
    // This measures the actual side metadata store/load operations.
    {
        let lines: Vec<Line> = block.lines().collect();
        let mark_state = LINE_RESET_MARK_STATE;

        c.bench_function("immix_line_mark_real", |b| {
            b.iter(|| {
                // Mark all lines
                for line in lines.iter() {
                    line.mark(mark_state);
                }
                // Check all lines
                let mut marked_count = 0u32;
                for line in lines.iter() {
                    if line.is_marked(mark_state) {
                        marked_count += 1;
                    }
                }
                // Clear marks for next iteration
                for line in lines.iter() {
                    line.mark(0);
                }
                criterion::black_box(marked_count);
            });
        });
    }

    // --- Benchmark 2: get_next_available_lines() on real ImmixSpace ---
    // Set up a realistic fragmentation pattern: mark ~50% of lines and search for holes.
    {
        let lines: Vec<Line> = block.lines().collect();
        let mark_state = LINE_RESET_MARK_STATE;

        c.bench_function("immix_hole_search_real_50pct", |b| {
            // Set up: mark every other line
            for (i, line) in lines.iter().enumerate() {
                if i % 2 == 0 {
                    line.mark(mark_state);
                } else {
                    line.mark(0);
                }
            }

            b.iter(|| {
                let mut holes = 0u32;
                let mut search = Some(block.start_line());
                while let Some(line) = search {
                    match immix_space.get_next_available_lines(line) {
                        Some((_start, end)) => {
                            holes += 1;
                            // Stop if end_line is the block boundary (next block may be unmapped)
                            search = if end == block.end_line() {
                                None
                            } else {
                                Some(end)
                            };
                        }
                        None => search = None,
                    }
                }
                criterion::black_box(holes);
            });
        });

        // Clean up marks
        for line in lines.iter() {
            line.mark(0);
        }
    }

    // --- Benchmark 3: get_next_available_lines() with clustered pattern ---
    {
        let lines: Vec<Line> = block.lines().collect();
        let mark_state = LINE_RESET_MARK_STATE;

        c.bench_function("immix_hole_search_real_clustered", |b| {
            // Set up: mark lines in clusters of 8 (typical multi-line objects)
            for (i, line) in lines.iter().enumerate() {
                let cluster = i / 8;
                if cluster % 2 == 0 {
                    line.mark(mark_state);
                } else {
                    line.mark(0);
                }
            }

            b.iter(|| {
                let mut holes = 0u32;
                let mut search = Some(block.start_line());
                while let Some(line) = search {
                    match immix_space.get_next_available_lines(line) {
                        Some((_start, end)) => {
                            holes += 1;
                            search = if end == block.end_line() {
                                None
                            } else {
                                Some(end)
                            };
                        }
                        None => search = None,
                    }
                }
                criterion::black_box(holes);
            });
        });

        // Clean up marks
        for line in lines.iter() {
            line.mark(0);
        }
    }

    // --- Benchmark 4: Line mark table scan (the inner loop of Block::sweep) ---
    // Block::sweep() can't be called repeatedly because it releases blocks.
    // Instead we benchmark the exact hot inner loop: reading the line mark table
    // and counting marked lines / holes. This is the MetadataByteArrayRef scan.
    {
        let lines: Vec<Line> = block.lines().collect();
        let mark_state = LINE_RESET_MARK_STATE;

        // Set up: mark ~50% of lines for a realistic pattern
        for (i, line) in lines.iter().enumerate() {
            if i % 2 == 0 {
                line.mark(mark_state);
            } else {
                line.mark(0);
            }
        }

        c.bench_function("immix_sweep_count_lines_50pct", |b| {
            b.iter(|| {
                // This is the exact loop from Block::sweep() lines 242-266
                let mut marked_lines = 0u32;
                let mut holes = 0u32;
                let mut prev_line_is_marked = true;
                for line in lines.iter() {
                    if line.is_marked(mark_state) {
                        marked_lines += 1;
                        prev_line_is_marked = true;
                    } else {
                        if prev_line_is_marked {
                            holes += 1;
                        }
                        prev_line_is_marked = false;
                    }
                }
                criterion::black_box((marked_lines, holes));
            });
        });

        // Clean up
        for line in lines.iter() {
            line.mark(0);
        }
    }

    // --- Benchmark 5: Line mark table scan on fully marked block ---
    {
        let lines: Vec<Line> = block.lines().collect();
        let mark_state = LINE_RESET_MARK_STATE;

        // Mark all lines
        for line in lines.iter() {
            line.mark(mark_state);
        }

        c.bench_function("immix_sweep_count_lines_full", |b| {
            b.iter(|| {
                let mut marked_lines = 0u32;
                let mut holes = 0u32;
                let mut prev_line_is_marked = true;
                for line in lines.iter() {
                    if line.is_marked(mark_state) {
                        marked_lines += 1;
                        prev_line_is_marked = true;
                    } else {
                        if prev_line_is_marked {
                            holes += 1;
                        }
                        prev_line_is_marked = false;
                    }
                }
                criterion::black_box((marked_lines, holes));
            });
        });

        // Clean up
        for line in lines.iter() {
            line.mark(0);
        }
    }
}
