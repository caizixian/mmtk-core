//! Benchmarks for object forwarding CAS patterns.
//!
//! These benchmarks simulate the atomic operations used in object forwarding
//! during copying GC. The key operation is `attempt_to_forward()` which uses
//! a CAS loop to atomically claim an object for forwarding.
//!
//! The forwarding word has three states:
//!   FORWARDING_NOT_TRIGGERED (0b00) → BEING_FORWARDED (0b10) → FORWARDED (0b11)
//!
//! These benchmarks measure the uncontended and lightly contended costs of
//! these state transitions.

use criterion::Criterion;
use std::sync::atomic::{AtomicU8, Ordering};

/// Forwarding state constants (matching object_forwarding.rs)
const FORWARDING_NOT_TRIGGERED: u8 = 0b00;
const BEING_FORWARDED: u8 = 0b10;
const FORWARDED: u8 = 0b11;

/// Simulate `attempt_to_forward()`: CAS from NOT_TRIGGERED to BEING_FORWARDED.
/// Returns the old forwarding state.
#[inline(always)]
fn attempt_to_forward(forwarding_word: &AtomicU8) -> u8 {
    loop {
        let old = forwarding_word.load(Ordering::SeqCst);
        if old != FORWARDING_NOT_TRIGGERED
            || forwarding_word
                .compare_exchange(old, BEING_FORWARDED, Ordering::SeqCst, Ordering::Relaxed)
                .is_ok()
        {
            return old;
        }
    }
}

/// Simulate completing forwarding: set to FORWARDED.
#[inline(always)]
fn complete_forwarding(forwarding_word: &AtomicU8) {
    forwarding_word.store(FORWARDED, Ordering::SeqCst);
}

/// Simulate reading forwarding status.
#[inline(always)]
fn get_forwarding_status(forwarding_word: &AtomicU8) -> u8 {
    forwarding_word.load(Ordering::SeqCst)
}

pub fn bench(c: &mut Criterion) {
    let num_objects = 4096;

    // Benchmark 1: Forwarding CAS on uncontended objects (all NOT_TRIGGERED → BEING_FORWARDED)
    c.bench_function("fwd_attempt_all_new", |b| {
        let objects: Vec<AtomicU8> = (0..num_objects)
            .map(|_| AtomicU8::new(FORWARDING_NOT_TRIGGERED))
            .collect();
        b.iter(|| {
            // Reset all to not triggered
            for obj in objects.iter() {
                obj.store(FORWARDING_NOT_TRIGGERED, Ordering::Relaxed);
            }
            let mut forwarded = 0u32;
            for obj in objects.iter() {
                let old = attempt_to_forward(obj);
                if old == FORWARDING_NOT_TRIGGERED {
                    forwarded += 1;
                    complete_forwarding(obj);
                }
            }
            assert_eq!(forwarded as usize, num_objects);
            criterion::black_box(forwarded);
        });
    });

    // Benchmark 2: Check forwarding status on already-forwarded objects
    // (simulating the case where we lost the forwarding race)
    c.bench_function("fwd_check_already_forwarded", |b| {
        let objects: Vec<AtomicU8> = (0..num_objects)
            .map(|_| AtomicU8::new(FORWARDED))
            .collect();
        b.iter(|| {
            let mut count = 0u32;
            for obj in objects.iter() {
                let status = get_forwarding_status(obj);
                if status == FORWARDED {
                    count += 1;
                }
            }
            assert_eq!(count as usize, num_objects);
            criterion::black_box(count);
        });
    });

    // Benchmark 3: Mixed forwarding — attempt_to_forward on objects where
    // some have already been forwarded (50% already forwarded).
    // This simulates the interleaving where some objects are processed by
    // another worker thread.
    c.bench_function("fwd_attempt_50pct_forwarded", |b| {
        let objects: Vec<AtomicU8> = (0..num_objects)
            .map(|i| {
                AtomicU8::new(if i % 2 == 0 {
                    FORWARDING_NOT_TRIGGERED
                } else {
                    FORWARDED
                })
            })
            .collect();
        b.iter(|| {
            // Reset alternating pattern
            for (i, obj) in objects.iter().enumerate() {
                obj.store(
                    if i % 2 == 0 {
                        FORWARDING_NOT_TRIGGERED
                    } else {
                        FORWARDED
                    },
                    Ordering::Relaxed,
                );
            }
            let mut newly_forwarded = 0u32;
            for obj in objects.iter() {
                let old = attempt_to_forward(obj);
                if old == FORWARDING_NOT_TRIGGERED {
                    newly_forwarded += 1;
                    complete_forwarding(obj);
                }
            }
            criterion::black_box(newly_forwarded);
        });
    });

    // Benchmark 4: Full forwarding pipeline — attempt + read forwarding pointer.
    // Simulates the complete forwarding hot path: CAS claim, then write forwarding
    // pointer (simulated as writing a usize to a separate array), then complete.
    c.bench_function("fwd_full_pipeline", |b| {
        let fwd_states: Vec<AtomicU8> = (0..num_objects)
            .map(|_| AtomicU8::new(FORWARDING_NOT_TRIGGERED))
            .collect();
        let mut fwd_pointers: Vec<usize> = vec![0; num_objects];

        b.iter(|| {
            // Reset
            for state in fwd_states.iter() {
                state.store(FORWARDING_NOT_TRIGGERED, Ordering::Relaxed);
            }
            for ptr in fwd_pointers.iter_mut() {
                *ptr = 0;
            }

            let mut forwarded = 0u32;
            for (i, state) in fwd_states.iter().enumerate() {
                let old = attempt_to_forward(state);
                if old == FORWARDING_NOT_TRIGGERED {
                    // Simulate writing forwarding pointer (new object address)
                    fwd_pointers[i] = i + num_objects; // fake new address
                    complete_forwarding(state);
                    forwarded += 1;
                }
            }
            criterion::black_box(forwarded);
        });
    });
}
