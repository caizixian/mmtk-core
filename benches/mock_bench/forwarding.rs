//! Mock benchmarks for object forwarding operations.
//!
//! These benchmarks call directly into MMTk's `object_forwarding` module:
//! - `attempt_to_forward()` (CAS from NOT_TRIGGERED to BEING_FORWARDED)
//! - `get_forwarding_status()` (read forwarding bits)
//! - `clear_forwarding_bits()` (reset forwarding state)
//!
//! Uses MockVM to allocate real objects with forwarding bits in the header.

use criterion::Criterion;

use mmtk::memory_manager;
use mmtk::util::test_private::{attempt_to_forward, clear_forwarding_bits, get_forwarding_status};
use mmtk::util::test_util::fixtures::*;
use mmtk::util::test_util::mock_method::*;
use mmtk::util::test_util::mock_vm::{write_mockvm, MockVM};
use mmtk::AllocationSemantics;

/// Allocate `count` objects of the given `size`.
fn allocate_objects(
    fixture: &mut MutatorFixture,
    count: usize,
    size: usize,
) -> Vec<mmtk::util::ObjectReference> {
    let mut objects = Vec::with_capacity(count);
    for _ in 0..count {
        let addr =
            memory_manager::alloc(&mut fixture.mutator, size, 8, 0, AllocationSemantics::Default);
        assert!(!addr.is_zero());
        let objref = MockVM::object_start_to_ref(addr);
        memory_manager::post_alloc(&mut fixture.mutator, objref, size, AllocationSemantics::Default);
        objects.push(objref);
    }
    objects
}

pub fn bench(c: &mut Criterion) {
    // Disable GC
    write_mockvm(|mock| {
        *mock = MockVM {
            is_collection_enabled: MockMethod::new_fixed(Box::new(|_| false)),
            ..MockVM::default()
        };
    });

    // Create fixture with generous heap
    let mut fixture = MutatorFixture::create_with_heapsize(256 * 1024 * 1024);

    // Allocate 4096 objects (64 bytes each)
    let objects = allocate_objects(&mut fixture, 4096, 64);

    // --- Benchmark 1: attempt_to_forward() on fresh objects ---
    // Each iteration: clear forwarding bits, then CAS forward all objects.
    c.bench_function("fwd_attempt_real_all_new", |b| {
        b.iter(|| {
            // Reset all forwarding bits
            for obj in objects.iter() {
                clear_forwarding_bits::<MockVM>(*obj);
            }
            let mut forwarded = 0u32;
            for obj in objects.iter() {
                let old = attempt_to_forward::<MockVM>(*obj);
                if old == 0 {
                    // FORWARDING_NOT_TRIGGERED_YET = 0b00
                    forwarded += 1;
                }
            }
            criterion::black_box(forwarded);
        });
    });

    // --- Benchmark 2: get_forwarding_status() on forwarded objects ---
    // Objects were left in BEING_FORWARDED state from bench 1.
    // Set them all to a known state first.
    for obj in objects.iter() {
        // CAS to BEING_FORWARDED if not already
        let _ = attempt_to_forward::<MockVM>(*obj);
    }

    c.bench_function("fwd_status_check_real", |b| {
        b.iter(|| {
            let mut count = 0u32;
            for obj in objects.iter() {
                let status = get_forwarding_status::<MockVM>(*obj);
                if status != 0 {
                    count += 1;
                }
            }
            criterion::black_box(count);
        });
    });

    // --- Benchmark 3: clear_forwarding_bits() ---
    c.bench_function("fwd_clear_real", |b| {
        b.iter(|| {
            // First forward them all
            for obj in objects.iter() {
                clear_forwarding_bits::<MockVM>(*obj);
                let _ = attempt_to_forward::<MockVM>(*obj);
            }
            // Then clear all
            for obj in objects.iter() {
                clear_forwarding_bits::<MockVM>(*obj);
            }
        });
    });

    // --- Benchmark 4: Forwarding race simulation ---
    // 50% of objects are already being forwarded (old == BEING_FORWARDED),
    // 50% are fresh (old == NOT_TRIGGERED).
    c.bench_function("fwd_attempt_real_50pct_forwarded", |b| {
        b.iter(|| {
            // Set up: alternate objects between forwarded and not-triggered
            for (i, obj) in objects.iter().enumerate() {
                clear_forwarding_bits::<MockVM>(*obj);
                if i % 2 == 0 {
                    // Leave as NOT_TRIGGERED
                } else {
                    // Forward this one
                    let _ = attempt_to_forward::<MockVM>(*obj);
                }
            }

            let mut newly_forwarded = 0u32;
            for obj in objects.iter() {
                let old = attempt_to_forward::<MockVM>(*obj);
                if old == 0 {
                    newly_forwarded += 1;
                }
            }
            criterion::black_box(newly_forwarded);
        });
    });
}
