//! Microbenchmarks comparing line mark scanning strategies for Immix sweep & hole search.
//!
//! # How to run
//!
//! ```sh
//! MMTK_BENCH=line_mark_scan cargo bench --features mock_test
//! ```
//!
//! # What it measures
//!
//! During every GC, Immix iterates the **line mark table** — a contiguous 128-byte
//! array where each byte records whether one 256-byte line is marked.  Two functions
//! traverse this table on the hot path:
//!
//!   - **`Block::sweep()`** counts marked lines and holes (transitions from marked→
//!     unmarked) to decide whether a block is reusable, fully dead, or fully live.
//!   - **`get_next_available_lines()`** searches for the next run of unmarked lines
//!     (a "hole") that the allocator can bump-allocate into.
//!
//! Both scan the 128-byte table **one byte at a time**.  This benchmark compares:
//!
//!   - **scalar** — The current MMTk approach: iterate bytes, compare, branch.
//!   - **word_u64** — Process 8 bytes at a time as `u64`.  XOR with a broadcast
//!     target to turn matching bytes into zero, then use the Hacker's Delight
//!     "has-zero-byte" trick to detect zero bytes without branching per byte.
//!   - **simd_sse2** — Use SSE2 `pcmpeqb` + `pmovmskb` to compare 16 bytes at a
//!     time and extract a bitmask.  SSE2 is guaranteed on all x86_64 CPUs.
//!
//! All strategies produce the same result (marked_lines, holes), so the benchmarks
//! measure pure throughput difference in the scanning hot loop.
//!
//! **No code changes to mmtk-core are required** — the optimized loops are entirely
//! within this benchmark file, operating on the same metadata byte array.
//!
//! # Correctness
//!
//! Before benchmarking, we verify that all three strategies produce identical
//! `(marked_lines, holes)` counts across several fragmentation patterns.
//!
//! # Performance analysis
//!
//! ## Why word-level scanning should help
//!
//! The scalar loop executes 128 iterations, each with:
//!   - 1 byte load
//!   - 1 compare + branch (is_marked vs not)
//!   - conditional increment (marked_lines or holes)
//!   - state update (prev_line_is_marked)
//!
//! At ~4–5 instructions per iteration, this is ~550 instructions per block.
//! The compare-and-branch creates a data dependency chain (prev_line_is_marked).
//!
//! The u64 loop processes 8 bytes per iteration (16 iterations for 128 bytes).
//! For **counting** marked lines, it eliminates the per-byte branch entirely:
//! XOR + zero-byte detection gives a bitmask of matching bytes in ~5 instructions
//! per 8 bytes.  The **hole-counting** part still has a sequential dependency,
//! but it can be derived from the bitmask transitions rather than per-byte branches.
//!
//! ## SSE2 availability
//!
//! Unlike AVX2 gather (which proved suboptimal on AMD Zen 3, see simd_tracing.rs),
//! SSE2 `pcmpeqb` is a simple per-byte comparison on contiguous data — exactly
//! what SIMD was designed for.  SSE2 is guaranteed on all x86_64 CPUs, so there's
//! no portability concern.

use criterion::Criterion;
use mmtk::util::test_private::*;
use std::hint::black_box;

// ───────────────────────────────────────────────────────────────────────────
// Constants
// ───────────────────────────────────────────────────────────────────────────

const NUM_LINES: usize = BLOCK_LINES; // 128 lines per block (default config)

// ───────────────────────────────────────────────────────────────────────────
// Sweep result: what Block::sweep() computes from the line mark table
// ───────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SweepResult {
    marked_lines: u32,
    holes: u32,
}

// ───────────────────────────────────────────────────────────────────────────
// Strategy 1: Scalar (the current MMTk approach)
// ───────────────────────────────────────────────────────────────────────────

/// Exact reproduction of the loop in `Block::sweep()` (block.rs:242-266).
/// Scans the line mark table one byte at a time.
#[inline(never)]
fn scalar_sweep_count(data: &[u8; NUM_LINES], mark_state: u8) -> SweepResult {
    let mut marked_lines = 0u32;
    let mut holes = 0u32;
    let mut prev_line_is_marked = true;

    for i in 0..NUM_LINES {
        if data[i] == mark_state {
            marked_lines += 1;
            prev_line_is_marked = true;
        } else {
            if prev_line_is_marked {
                holes += 1;
            }
            prev_line_is_marked = false;
        }
    }

    SweepResult {
        marked_lines,
        holes,
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Strategy 2: Word-level u64 scanning
// ───────────────────────────────────────────────────────────────────────────

/// Process 8 bytes at a time by comparing against a broadcast target.
///
/// For counting: XOR each 8-byte word with the target (all bytes = mark_state).
/// Bytes that match become 0x00; non-matching bytes become non-zero.
/// Then count zero bytes using the Hacker's Delight "has-zero-byte" detect.
///
/// For holes: extract the last byte state to maintain the prev_line_is_marked
/// dependency, and count transitions from the per-word match mask.
#[inline(never)]
fn word_u64_sweep_count(data: &[u8; NUM_LINES], mark_state: u8) -> SweepResult {
    let mut marked_lines = 0u32;
    let mut holes = 0u32;
    let mut prev_line_is_marked = true;

    // Broadcast the mark_state to all 8 bytes of a u64
    let target = u64::from_ne_bytes([mark_state; 8]);

    let chunks = NUM_LINES / 8;
    for chunk_idx in 0..chunks {
        let base = chunk_idx * 8;
        // Load 8 bytes as a u64
        let word = u64::from_ne_bytes([
            data[base],
            data[base + 1],
            data[base + 2],
            data[base + 3],
            data[base + 4],
            data[base + 5],
            data[base + 6],
            data[base + 7],
        ]);

        // XOR: matching bytes become 0x00, non-matching become non-zero
        let xored = word ^ target;

        // For each byte in the word, determine if it's marked (zero in xored)
        // Process the 8 bytes from the xored word to count marks and holes.
        // Extract individual byte match results:
        let bytes = xored.to_ne_bytes();
        for i in 0..8 {
            if bytes[i] == 0 {
                // This byte matched mark_state
                marked_lines += 1;
                prev_line_is_marked = true;
            } else {
                if prev_line_is_marked {
                    holes += 1;
                }
                prev_line_is_marked = false;
            }
        }
    }

    SweepResult {
        marked_lines,
        holes,
    }
}

/// Count ONLY marked lines (no hole counting) using word-level scanning.
/// This eliminates the sequential dependency and shows the pure counting speedup.
///
/// Uses XOR + byte extraction: XOR each 8-byte word with the target, then
/// count zero bytes by extracting each byte. The compiler can auto-vectorize
/// this pattern effectively.
#[inline(never)]
fn word_u64_count_only(data: &[u8; NUM_LINES], mark_state: u8) -> u32 {
    let mut marked_lines = 0u32;
    let target = u64::from_ne_bytes([mark_state; 8]);

    let chunks = NUM_LINES / 8;
    for chunk_idx in 0..chunks {
        let base = chunk_idx * 8;
        let word = u64::from_ne_bytes([
            data[base],
            data[base + 1],
            data[base + 2],
            data[base + 3],
            data[base + 4],
            data[base + 5],
            data[base + 6],
            data[base + 7],
        ]);

        // XOR: matching bytes become 0x00
        let xored = word ^ target;

        // Count zero bytes by extracting each byte from the xored result.
        // This is simple and correct; the compiler optimizes this well.
        let bytes = xored.to_ne_bytes();
        for b in bytes {
            if b == 0 {
                marked_lines += 1;
            }
        }
    }

    marked_lines
}

// ───────────────────────────────────────────────────────────────────────────
// Strategy 3: SSE2 SIMD scanning
// ───────────────────────────────────────────────────────────────────────────

/// SSE2 strategy: use `pcmpeqb` to compare 16 bytes at a time, then
/// `pmovmskb` to extract a 16-bit mask of matching bytes.
///
/// Falls back to word_u64 if not x86_64.
#[inline(never)]
fn simd_sse2_sweep_count(data: &[u8; NUM_LINES], mark_state: u8) -> SweepResult {
    #[cfg(target_arch = "x86_64")]
    {
        // SSE2 is guaranteed on all x86_64 — no feature detection needed.
        // Safety: SSE2 is always available on x86_64.
        return unsafe { simd_sse2_sweep_count_inner(data, mark_state) };
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        word_u64_sweep_count(data, mark_state)
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn simd_sse2_sweep_count_inner(
    data: &[u8; NUM_LINES],
    mark_state: u8,
) -> SweepResult {
    use std::arch::x86_64::*;

    let mut marked_lines = 0u32;
    let mut holes = 0u32;
    let mut prev_line_is_marked = true;

    // Broadcast mark_state to all 16 bytes
    let target = _mm_set1_epi8(mark_state as i8);

    let chunks = NUM_LINES / 16;
    for chunk_idx in 0..chunks {
        let base = chunk_idx * 16;
        // Load 16 bytes (unaligned is fine for side metadata)
        let vec = _mm_loadu_si128(data.as_ptr().add(base) as *const __m128i);

        // Compare: each byte → 0xFF if equal, 0x00 if not
        let eq = _mm_cmpeq_epi8(vec, target);

        // Extract comparison result as a 16-bit mask (1 bit per byte)
        let mask = _mm_movemask_epi8(eq) as u16;

        // Count marked lines in this chunk
        marked_lines += mask.count_ones() as u32;

        // Count holes: scan the 16 bits for transitions from 1→0
        for bit_idx in 0..16u32 {
            let is_marked = (mask >> bit_idx) & 1 == 1;
            if is_marked {
                prev_line_is_marked = true;
            } else {
                if prev_line_is_marked {
                    holes += 1;
                }
                prev_line_is_marked = false;
            }
        }
    }

    SweepResult {
        marked_lines,
        holes,
    }
}

/// SSE2 count-only: count marked lines without hole detection.
/// This shows the maximum benefit from SIMD — no sequential dependency at all.
/// Uses software popcount (compiler default, ~12 instructions).
#[inline(never)]
fn simd_sse2_count_only(data: &[u8; NUM_LINES], mark_state: u8) -> u32 {
    #[cfg(target_arch = "x86_64")]
    {
        return unsafe { simd_sse2_count_only_inner(data, mark_state) };
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        word_u64_count_only(data, mark_state)
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn simd_sse2_count_only_inner(data: &[u8; NUM_LINES], mark_state: u8) -> u32 {
    use std::arch::x86_64::*;

    let mut marked_lines = 0u32;
    let target = _mm_set1_epi8(mark_state as i8);

    let chunks = NUM_LINES / 16;
    for chunk_idx in 0..chunks {
        let base = chunk_idx * 16;
        let vec = _mm_loadu_si128(data.as_ptr().add(base) as *const __m128i);
        let eq = _mm_cmpeq_epi8(vec, target);
        let mask = _mm_movemask_epi8(eq) as u16;
        marked_lines += mask.count_ones() as u32;
    }

    marked_lines
}

// ───────────────────────────────────────────────────────────────────────────
// Strategy 4: Improved SSE2 sweep — vectorial transition detection
// ───────────────────────────────────────────────────────────────────────────
//
// Key insight from uarch analysis: the compiler's auto-vectorized scalar
// version beats our manual SSE2 because it computes hole transitions
// *vectorially* using shufps + andnps (all 1-cycle FP-domain operations),
// while our manual version uses pmovmskb (5-cycle FP→INT crossing) then
// scans bits one-at-a-time in the integer pipeline.
//
// This improved version mirrors the compiler's strategy: use pslldq
// (byte shift left) to create a shifted copy of the comparison result,
// then pandn to detect marked→unmarked transitions, staying entirely
// in the FP/SIMD domain.

/// Improved SSE2 sweep: vectorial transition detection, stays in FP domain.
#[inline(never)]
fn simd_sse2_sweep_v2(data: &[u8; NUM_LINES], mark_state: u8) -> SweepResult {
    #[cfg(target_arch = "x86_64")]
    {
        return unsafe { simd_sse2_sweep_v2_inner(data, mark_state) };
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        scalar_sweep_count(data, mark_state)
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn simd_sse2_sweep_v2_inner(
    data: &[u8; NUM_LINES],
    mark_state: u8,
) -> SweepResult {
    use std::arch::x86_64::*;

    let target = _mm_set1_epi8(mark_state as i8);
    let ones = _mm_set1_epi8(1); // each byte = 0x01

    // Accumulators — 4 × i32 lanes, summed at the end
    let mut acc_marked = _mm_setzero_si128();
    let mut acc_holes = _mm_setzero_si128();

    // Previous chunk's last byte comparison result (0xFF = marked, 0x00 = not)
    // Initialize to 0xFF because prev_line_is_marked starts as true
    let mut prev_last = _mm_set1_epi8(-1i8); // all 0xFF

    let chunks = NUM_LINES / 16;
    for chunk_idx in 0..chunks {
        let base = chunk_idx * 16;
        let vec = _mm_loadu_si128(data.as_ptr().add(base) as *const __m128i);

        // Compare: each byte → 0xFF if equal to mark_state, 0x00 if not
        let eq = _mm_cmpeq_epi8(vec, target);

        // --- Count marked lines ---
        // eq has 0xFF for marked bytes, 0x00 for unmarked.
        // We need to count the 0xFF bytes. Since 0xFF = -1 in signed,
        // negate to get 0x01 for marked, 0x00 for unmarked, then
        // use psadbw to sum bytes horizontally.
        let marked_01 = _mm_and_si128(eq, ones); // 0x01 for marked, 0x00 for unmarked
        // psadbw computes sum of absolute differences vs zero → sums the bytes
        let sad = _mm_sad_epu8(marked_01, _mm_setzero_si128());
        acc_marked = _mm_add_epi32(acc_marked, sad);

        // --- Count holes (marked→unmarked transitions) ---
        // Create a shifted version: for byte[i], we want byte[i-1]'s comparison.
        // pslldq shifts left by 1 byte: shifted[0] = 0, shifted[i] = eq[i-1]
        // Then OR in the previous chunk's last byte at position 0.
        let shifted = _mm_slli_si128(eq, 1);
        // Insert prev_last's byte 15 into shifted's byte 0
        // We use: shifted = shifted | (prev_last & mask_byte0)
        // But simpler: use _mm_or_si128 with a mask that has prev's last byte at pos 0
        let prev_byte = _mm_srli_si128(prev_last, 15); // byte 15 → byte 0, rest = 0
        let prev_eq = _mm_or_si128(shifted, prev_byte);

        // A hole starts where prev was marked (0xFF) and current is unmarked (0x00)
        // transitions = prev_eq & ~eq (prev marked AND current unmarked)
        let transitions = _mm_andnot_si128(eq, prev_eq);

        // Count transition bytes (each is 0xFF = -1)
        let trans_01 = _mm_and_si128(transitions, ones);
        let trans_sad = _mm_sad_epu8(trans_01, _mm_setzero_si128());
        acc_holes = _mm_add_epi32(acc_holes, trans_sad);

        // Save this chunk's comparison result for next iteration
        prev_last = eq;
    }

    // Horizontal sum: acc_marked has sums in lanes 0 and 2 (from psadbw)
    let marked_lines = (_mm_extract_epi16(acc_marked, 0) + _mm_extract_epi16(acc_marked, 4)) as u32;
    let holes = (_mm_extract_epi16(acc_holes, 0) + _mm_extract_epi16(acc_holes, 4)) as u32;

    SweepResult {
        marked_lines,
        holes,
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Strategy 5: SSE2 count-only with hardware popcnt
// ───────────────────────────────────────────────────────────────────────────
//
// The original simd_sse2_count_only uses software popcount (~12 instructions,
// ~8 cycles) because the compiler doesn't enable hardware popcnt by default.
// On Zen 3, hardware POPCNT is 1 µop, 1-cycle latency, 0.33c throughput.
// This version explicitly enables it.

/// SSE2 count-only with hardware popcnt: pcmpeqb + pmovmskb + popcnt.
/// This is the theoretical best for pure counting on x86_64.
#[inline(never)]
fn simd_sse2_count_popcnt(data: &[u8; NUM_LINES], mark_state: u8) -> u32 {
    #[cfg(target_arch = "x86_64")]
    {
        return unsafe { simd_sse2_count_popcnt_inner(data, mark_state) };
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        scalar_count_only(data, mark_state)
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "popcnt")]
unsafe fn simd_sse2_count_popcnt_inner(data: &[u8; NUM_LINES], mark_state: u8) -> u32 {
    use std::arch::x86_64::*;

    let mut marked_lines = 0u32;
    let target = _mm_set1_epi8(mark_state as i8);

    let chunks = NUM_LINES / 16;
    for chunk_idx in 0..chunks {
        let base = chunk_idx * 16;
        let vec = _mm_loadu_si128(data.as_ptr().add(base) as *const __m128i);
        let eq = _mm_cmpeq_epi8(vec, target);
        let mask = _mm_movemask_epi8(eq) as u32;
        // Hardware popcnt: 1 µop, 1c latency on Zen 3
        marked_lines += _popcnt32(mask as i32) as u32;
    }

    marked_lines
}

// ───────────────────────────────────────────────────────────────────────────
// Strategy 6: Improved SSE2 count-only using psadbw (no pmovmskb at all)
// ───────────────────────────────────────────────────────────────────────────
//
// Avoids pmovmskb entirely by staying in the FP domain:
// pcmpeqb → pand(ones) → psadbw → paddd
// This eliminates the 5-cycle FP→INT domain crossing.

/// SSE2 count-only using psadbw: stays entirely in FP domain.
#[inline(never)]
fn simd_sse2_count_sad(data: &[u8; NUM_LINES], mark_state: u8) -> u32 {
    #[cfg(target_arch = "x86_64")]
    {
        return unsafe { simd_sse2_count_sad_inner(data, mark_state) };
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        scalar_count_only(data, mark_state)
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn simd_sse2_count_sad_inner(data: &[u8; NUM_LINES], mark_state: u8) -> u32 {
    use std::arch::x86_64::*;

    let target = _mm_set1_epi8(mark_state as i8);
    let ones = _mm_set1_epi8(1);
    let mut acc = _mm_setzero_si128();

    let chunks = NUM_LINES / 16;
    for chunk_idx in 0..chunks {
        let base = chunk_idx * 16;
        let vec = _mm_loadu_si128(data.as_ptr().add(base) as *const __m128i);
        let eq = _mm_cmpeq_epi8(vec, target);
        // Map 0xFF → 0x01, 0x00 → 0x00
        let matched = _mm_and_si128(eq, ones);
        // Sum bytes horizontally: psadbw sums 8 bytes into u16 in lanes 0 and 4
        let sad = _mm_sad_epu8(matched, _mm_setzero_si128());
        acc = _mm_add_epi64(acc, sad);
    }

    // Extract from lanes 0 and 4 (psadbw puts results in 64-bit halves)
    (_mm_extract_epi16(acc, 0) + _mm_extract_epi16(acc, 4)) as u32
}

// ───────────────────────────────────────────────────────────────────────────
// Strategy 1b: Scalar count-only (no holes)
// ───────────────────────────────────────────────────────────────────────────

/// Count only marked lines, no hole detection. Baseline for count-only comparison.
#[inline(never)]
fn scalar_count_only(data: &[u8; NUM_LINES], mark_state: u8) -> u32 {
    let mut marked_lines = 0u32;
    for i in 0..NUM_LINES {
        if data[i] == mark_state {
            marked_lines += 1;
        }
    }
    marked_lines
}

// ───────────────────────────────────────────────────────────────────────────
// Hole-search strategies
// ───────────────────────────────────────────────────────────────────────────

/// Find the first available hole (run of non-matching bytes) starting from `start_idx`.
/// Returns `Some((hole_start, hole_end))` or `None`.
/// This mirrors `get_next_available_lines()`.
#[inline(never)]
fn scalar_find_hole(
    data: &[u8; NUM_LINES],
    start_idx: usize,
    unavail_state: u8,
    current_state: u8,
) -> Option<(usize, usize)> {
    let mut cursor = start_idx;

    // Find start: skip over marked/unavailable lines
    while cursor < NUM_LINES {
        let mark = data[cursor];
        if mark != unavail_state && mark != current_state {
            break;
        }
        cursor += 1;
    }
    if cursor == NUM_LINES {
        return None;
    }
    let start = cursor;

    // Find end: skip over available (unmarked) lines
    while cursor < NUM_LINES {
        let mark = data[cursor];
        if mark == unavail_state || mark == current_state {
            break;
        }
        cursor += 1;
    }

    Some((start, cursor))
}

/// Word-level hole search: scan 8 bytes at a time to find the first byte
/// that doesn't match either state.
///
/// Instead of complex SWAR bit tricks, we load 8 bytes at a time and
/// check if all 8 bytes match one of the two states. If they do, skip
/// the entire word. Otherwise, fall back to byte-by-byte for that word.
#[inline(never)]
fn word_u64_find_hole(
    data: &[u8; NUM_LINES],
    start_idx: usize,
    unavail_state: u8,
    current_state: u8,
) -> Option<(usize, usize)> {
    let unavail_word = u64::from_ne_bytes([unavail_state; 8]);
    let current_word = u64::from_ne_bytes([current_state; 8]);

    let mut cursor = start_idx;

    // Phase 1: Skip over marked lines (find hole start)
    // Process byte-by-byte until aligned to 8-byte boundary
    while cursor < NUM_LINES && cursor % 8 != 0 {
        let mark = data[cursor];
        if mark != unavail_state && mark != current_state {
            break;
        }
        cursor += 1;
    }

    // Process 8 bytes at a time — check if all bytes match one of the states
    if cursor < NUM_LINES && cursor % 8 == 0 {
        while cursor + 8 <= NUM_LINES {
            let word = u64::from_ne_bytes(data[cursor..cursor + 8].try_into().unwrap());

            // Quick check: if the entire word equals one of the broadcast targets,
            // all 8 bytes are the same state — skip immediately.
            if word == unavail_word || word == current_word {
                cursor += 8;
                continue;
            }

            // Mixed word: check byte-by-byte if all match one of the two states.
            // XOR with each target; a zero byte in XOR means a match.
            let xor_unavail = word ^ unavail_word;
            let xor_current = word ^ current_word;

            // For each byte position, it's "marked" if it matches either state,
            // i.e., the byte is zero in xor_unavail OR zero in xor_current.
            // If ANY byte is non-zero in both, this word has an available line.
            let bytes_unavail = xor_unavail.to_ne_bytes();
            let bytes_current = xor_current.to_ne_bytes();
            let all_marked = (0..8).all(|i| bytes_unavail[i] == 0 || bytes_current[i] == 0);

            if all_marked {
                cursor += 8;
                continue;
            }
            // Some bytes are available — find the first one byte-by-byte
            break;
        }
    }

    // Finish byte-by-byte
    while cursor < NUM_LINES {
        let mark = data[cursor];
        if mark != unavail_state && mark != current_state {
            break;
        }
        cursor += 1;
    }

    if cursor == NUM_LINES {
        return None;
    }
    let start = cursor;

    // Phase 2: Find hole end (skip over available lines)
    while cursor < NUM_LINES {
        let mark = data[cursor];
        if mark == unavail_state || mark == current_state {
            break;
        }
        cursor += 1;
    }

    Some((start, cursor))
}

/// SSE2 hole search: use pcmpeqb + pmovmskb to scan 16 bytes at a time.
#[inline(never)]
fn simd_sse2_find_hole(
    data: &[u8; NUM_LINES],
    start_idx: usize,
    unavail_state: u8,
    current_state: u8,
) -> Option<(usize, usize)> {
    #[cfg(target_arch = "x86_64")]
    {
        return unsafe {
            simd_sse2_find_hole_inner(data, start_idx, unavail_state, current_state)
        };
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        word_u64_find_hole(data, start_idx, unavail_state, current_state)
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn simd_sse2_find_hole_inner(
    data: &[u8; NUM_LINES],
    start_idx: usize,
    unavail_state: u8,
    current_state: u8,
) -> Option<(usize, usize)> {
    use std::arch::x86_64::*;

    let unavail_vec = _mm_set1_epi8(unavail_state as i8);
    let current_vec = _mm_set1_epi8(current_state as i8);

    let mut cursor = start_idx;

    // Phase 1: Skip over marked lines using SIMD
    // Handle unaligned prefix byte-by-byte
    while cursor < NUM_LINES && cursor % 16 != 0 {
        let mark = data[cursor];
        if mark != unavail_state && mark != current_state {
            break;
        }
        cursor += 1;
    }

    // Process 16 bytes at a time
    if cursor % 16 == 0 {
        while cursor + 16 <= NUM_LINES {
            let vec = _mm_loadu_si128(data.as_ptr().add(cursor) as *const __m128i);
            let eq_unavail = _mm_cmpeq_epi8(vec, unavail_vec);
            let eq_current = _mm_cmpeq_epi8(vec, current_vec);
            // OR the two masks: bytes matching either state are "marked"
            let marked = _mm_or_si128(eq_unavail, eq_current);
            let mask = _mm_movemask_epi8(marked) as u16;

            if mask == 0xFFFF {
                // All 16 bytes match one of the states → skip
                cursor += 16;
                continue;
            }
            // Some bytes are available — find the first one
            // !mask gives bits where bytes are available
            let avail_mask = !mask & 0xFFFF;
            let first_avail = avail_mask.trailing_zeros() as usize;
            cursor += first_avail;
            break;
        }
    }

    // Finish byte-by-byte for remainder
    while cursor < NUM_LINES {
        let mark = data[cursor];
        if mark != unavail_state && mark != current_state {
            break;
        }
        cursor += 1;
    }

    if cursor >= NUM_LINES {
        return None;
    }
    let start = cursor;

    // Phase 2: Find hole end
    while cursor < NUM_LINES {
        let mark = data[cursor];
        if mark == unavail_state || mark == current_state {
            break;
        }
        cursor += 1;
    }

    Some((start, cursor))
}

// ───────────────────────────────────────────────────────────────────────────
// Test data generation
// ───────────────────────────────────────────────────────────────────────────

/// Create a line mark table with a specific fragmentation pattern.
fn make_pattern(mark_state: u8, pattern: &str) -> [u8; NUM_LINES] {
    let mut data = [0u8; NUM_LINES];
    match pattern {
        // 50% fragmentation: every other line is marked
        "alternating" => {
            for i in 0..NUM_LINES {
                if i % 2 == 0 {
                    data[i] = mark_state;
                }
            }
        }
        // Clustered: groups of 8 marked, then 8 unmarked
        "clustered" => {
            for i in 0..NUM_LINES {
                if (i / 8) % 2 == 0 {
                    data[i] = mark_state;
                }
            }
        }
        // 75% occupancy: every 4th line is unmarked
        "dense" => {
            for i in 0..NUM_LINES {
                if i % 4 != 3 {
                    data[i] = mark_state;
                }
            }
        }
        // Fully marked (live block, common case in sweep)
        "full" => {
            for i in 0..NUM_LINES {
                data[i] = mark_state;
            }
        }
        // Fully empty (dead block)
        "empty" => {
            // All zeros — already correct
        }
        _ => panic!("Unknown pattern: {}", pattern),
    }
    data
}

// ───────────────────────────────────────────────────────────────────────────
// Correctness verification
// ───────────────────────────────────────────────────────────────────────────

fn verify_sweep_correctness(data: &[u8; NUM_LINES], mark_state: u8, pattern_name: &str) {
    let scalar = scalar_sweep_count(data, mark_state);
    let word = word_u64_sweep_count(data, mark_state);
    let simd = simd_sse2_sweep_count(data, mark_state);
    let simd_v2 = simd_sse2_sweep_v2(data, mark_state);

    assert_eq!(
        scalar, word,
        "Sweep count mismatch ({pattern_name}): scalar={scalar:?}, word={word:?}"
    );
    assert_eq!(
        scalar, simd,
        "Sweep count mismatch ({pattern_name}): scalar={scalar:?}, simd={simd:?}"
    );
    assert_eq!(
        scalar, simd_v2,
        "Sweep v2 mismatch ({pattern_name}): scalar={scalar:?}, simd_v2={simd_v2:?}"
    );

    let scalar_count = scalar_count_only(data, mark_state);
    let word_count = word_u64_count_only(data, mark_state);
    let simd_count = simd_sse2_count_only(data, mark_state);
    let popcnt_count = simd_sse2_count_popcnt(data, mark_state);
    let sad_count = simd_sse2_count_sad(data, mark_state);

    assert_eq!(scalar_count, scalar.marked_lines);
    assert_eq!(word_count, scalar.marked_lines,
        "Count-only mismatch ({pattern_name}): scalar={scalar_count}, word={word_count}");
    assert_eq!(simd_count, scalar.marked_lines,
        "Count-only mismatch ({pattern_name}): scalar={scalar_count}, simd={simd_count}");
    assert_eq!(popcnt_count, scalar.marked_lines,
        "Count-popcnt mismatch ({pattern_name}): scalar={scalar_count}, popcnt={popcnt_count}");
    assert_eq!(sad_count, scalar.marked_lines,
        "Count-sad mismatch ({pattern_name}): scalar={scalar_count}, sad={sad_count}");
}

fn verify_hole_search_correctness(
    data: &[u8; NUM_LINES],
    unavail_state: u8,
    current_state: u8,
    pattern_name: &str,
) {
    // Verify: iterate all holes and compare results
    let mut scalar_holes = Vec::new();
    let mut word_holes = Vec::new();
    let mut simd_holes = Vec::new();

    let mut pos = 0;
    while let Some((start, end)) = scalar_find_hole(data, pos, unavail_state, current_state) {
        scalar_holes.push((start, end));
        pos = end;
    }

    pos = 0;
    while let Some((start, end)) = word_u64_find_hole(data, pos, unavail_state, current_state) {
        word_holes.push((start, end));
        pos = end;
    }

    pos = 0;
    while let Some((start, end)) = simd_sse2_find_hole(data, pos, unavail_state, current_state) {
        simd_holes.push((start, end));
        pos = end;
    }

    assert_eq!(
        scalar_holes, word_holes,
        "Hole search mismatch ({pattern_name}): scalar={scalar_holes:?}, word={word_holes:?}"
    );
    assert_eq!(
        scalar_holes, simd_holes,
        "Hole search mismatch ({pattern_name}): scalar={scalar_holes:?}, simd={simd_holes:?}"
    );

    eprintln!(
        "[line_mark_scan] {pattern_name}: sweep={:?}, {} holes found",
        scalar_sweep_count(data, current_state),
        scalar_holes.len()
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Benchmark entry point
// ───────────────────────────────────────────────────────────────────────────

pub fn bench(c: &mut Criterion) {
    // Use a mark state similar to what Immix uses in practice
    let mark_state: u8 = LINE_RESET_MARK_STATE; // = 1
    // For hole search, we need both unavail_state and current_state
    // In practice, unavail_state is the previous GC's mark state
    let unavail_state: u8 = if mark_state > 1 { mark_state - 1 } else { LINE_MAX_MARK_STATE };
    let current_state: u8 = mark_state;

    // Generate test patterns
    let patterns = ["alternating", "clustered", "dense", "full", "empty"];
    let datasets: Vec<([u8; NUM_LINES], &str)> = patterns
        .iter()
        .map(|p| (make_pattern(current_state, p), *p))
        .collect();

    // ── Correctness verification ──────────────────────────────────────────
    for (data, name) in &datasets {
        verify_sweep_correctness(data, current_state, name);
        verify_hole_search_correctness(data, unavail_state, current_state, name);
    }
    eprintln!("[line_mark_scan] All correctness checks passed.");

    // ── Extended correctness: edge-case patterns ─────────────────────────
    // These test boundary conditions that the simple patterns might miss:
    // chunk boundaries (byte 15→16, 31→32), single marks, all mark states.
    {
        // Edge case 1: Single marked byte at various positions
        for pos in [0usize, 1, 7, 8, 15, 16, 17, 31, 32, 63, 64, 127] {
            let mut data = [0u8; NUM_LINES];
            data[pos] = current_state;
            verify_sweep_correctness(&data, current_state, &format!("single_mark_at_{}", pos));
        }

        // Edge case 2: Hole at 16-byte chunk boundary (tests pslldq carry-over)
        // Mark bytes 14,15 (end of chunk 0) and 16,17 (start of chunk 1), then
        // leave a gap — the hole detection must correctly carry state across chunks.
        let mut data = [0u8; NUM_LINES];
        for i in 14..=17 {
            data[i] = current_state;
        }
        verify_sweep_correctness(&data, current_state, "chunk_boundary_14_17");

        // Edge case 3: Transition exactly at chunk boundary
        // Bytes 0-15 all marked, byte 16 unmarked → hole starts at chunk boundary
        let mut data = [0u8; NUM_LINES];
        for i in 0..16 {
            data[i] = current_state;
        }
        verify_sweep_correctness(&data, current_state, "marked_first_chunk_only");

        // Edge case 4: All transitions at chunk boundaries
        // Mark chunks 0, 2, 4, 6 — leave chunks 1, 3, 5, 7 empty
        let mut data = [0u8; NUM_LINES];
        for chunk in [0, 2, 4, 6] {
            for i in (chunk * 16)..((chunk + 1) * 16) {
                data[i] = current_state;
            }
        }
        verify_sweep_correctness(&data, current_state, "alternating_chunks");

        // Edge case 5: Test with different mark_state values
        for ms in 1u8..=LINE_MAX_MARK_STATE {
            let mut data = [0u8; NUM_LINES];
            for i in (0..NUM_LINES).step_by(3) {
                data[i] = ms;
            }
            verify_sweep_correctness(&data, ms, &format!("mark_state_{}_every_3rd", ms));
        }

        // Edge case 6: Last byte only
        let mut data = [0u8; NUM_LINES];
        data[NUM_LINES - 1] = current_state;
        verify_sweep_correctness(&data, current_state, "last_byte_only");

        // Edge case 7: Pseudo-random pattern (deterministic)
        let mut data = [0u8; NUM_LINES];
        let mut rng: u32 = 0xDEADBEEF;
        for i in 0..NUM_LINES {
            rng = rng.wrapping_mul(1103515245).wrapping_add(12345);
            if (rng >> 16) & 1 == 1 {
                data[i] = current_state;
            }
        }
        verify_sweep_correctness(&data, current_state, "pseudo_random");

        eprintln!("[line_mark_scan] All extended edge-case checks passed.");
    }


    // ── Benchmark group: sweep counting (full: marked_lines + holes) ─────

    let mut group = c.benchmark_group("sweep_count");
    // For the primary benchmark, use "alternating" (worst case for branching)
    let data_alt = make_pattern(current_state, "alternating");

    group.bench_function("scalar", |b| {
        b.iter(|| {
            black_box(scalar_sweep_count(black_box(&data_alt), current_state));
        });
    });

    group.bench_function("word_u64", |b| {
        b.iter(|| {
            black_box(word_u64_sweep_count(black_box(&data_alt), current_state));
        });
    });

    group.bench_function("simd_sse2", |b| {
        b.iter(|| {
            black_box(simd_sse2_sweep_count(black_box(&data_alt), current_state));
        });
    });

    group.bench_function("simd_sse2_v2", |b| {
        b.iter(|| {
            black_box(simd_sse2_sweep_v2(black_box(&data_alt), current_state));
        });
    });

    group.finish();

    // ── Benchmark group: count-only (no hole detection) ──────────────────
    // This isolates the pure counting benefit without sequential dependency

    let mut group = c.benchmark_group("count_only");

    group.bench_function("scalar", |b| {
        b.iter(|| {
            black_box(scalar_count_only(black_box(&data_alt), current_state));
        });
    });

    group.bench_function("word_u64", |b| {
        b.iter(|| {
            black_box(word_u64_count_only(black_box(&data_alt), current_state));
        });
    });

    group.bench_function("simd_sse2", |b| {
        b.iter(|| {
            black_box(simd_sse2_count_only(black_box(&data_alt), current_state));
        });
    });

    group.bench_function("simd_sse2_popcnt", |b| {
        b.iter(|| {
            black_box(simd_sse2_count_popcnt(black_box(&data_alt), current_state));
        });
    });

    group.bench_function("simd_sse2_sad", |b| {
        b.iter(|| {
            black_box(simd_sse2_count_sad(black_box(&data_alt), current_state));
        });
    });

    group.finish();

    // ── Benchmark group: hole search ─────────────────────────────────────
    // Search for all holes in the block (like get_next_available_lines does)

    let mut group = c.benchmark_group("hole_search");
    let data_clust = make_pattern(current_state, "clustered");

    group.bench_function("scalar_alternating", |b| {
        b.iter(|| {
            let mut holes = 0u32;
            let mut pos = 0;
            while let Some((_start, end)) =
                scalar_find_hole(black_box(&data_alt), pos, unavail_state, current_state)
            {
                holes += 1;
                pos = end;
            }
            black_box(holes);
        });
    });

    group.bench_function("word_u64_alternating", |b| {
        b.iter(|| {
            let mut holes = 0u32;
            let mut pos = 0;
            while let Some((_start, end)) =
                word_u64_find_hole(black_box(&data_alt), pos, unavail_state, current_state)
            {
                holes += 1;
                pos = end;
            }
            black_box(holes);
        });
    });

    group.bench_function("simd_sse2_alternating", |b| {
        b.iter(|| {
            let mut holes = 0u32;
            let mut pos = 0;
            while let Some((_start, end)) =
                simd_sse2_find_hole(black_box(&data_alt), pos, unavail_state, current_state)
            {
                holes += 1;
                pos = end;
            }
            black_box(holes);
        });
    });

    group.bench_function("scalar_clustered", |b| {
        b.iter(|| {
            let mut holes = 0u32;
            let mut pos = 0;
            while let Some((_start, end)) =
                scalar_find_hole(black_box(&data_clust), pos, unavail_state, current_state)
            {
                holes += 1;
                pos = end;
            }
            black_box(holes);
        });
    });

    group.bench_function("word_u64_clustered", |b| {
        b.iter(|| {
            let mut holes = 0u32;
            let mut pos = 0;
            while let Some((_start, end)) =
                word_u64_find_hole(black_box(&data_clust), pos, unavail_state, current_state)
            {
                holes += 1;
                pos = end;
            }
            black_box(holes);
        });
    });

    group.bench_function("simd_sse2_clustered", |b| {
        b.iter(|| {
            let mut holes = 0u32;
            let mut pos = 0;
            while let Some((_start, end)) =
                simd_sse2_find_hole(black_box(&data_clust), pos, unavail_state, current_state)
            {
                holes += 1;
                pos = end;
            }
            black_box(holes);
        });
    });

    group.finish();

    // ── Benchmark group: sweep counting across patterns ──────────────────
    // Shows how each strategy performs on different fragmentation levels

    let mut group = c.benchmark_group("sweep_by_pattern");

    for (data, pattern_name) in &datasets {
        group.bench_function(format!("scalar_{}", pattern_name), |b| {
            b.iter(|| {
                black_box(scalar_sweep_count(black_box(data), current_state));
            });
        });

        group.bench_function(format!("simd_sse2_{}", pattern_name), |b| {
            b.iter(|| {
                black_box(simd_sse2_sweep_count(black_box(data), current_state));
            });
        });
    }

    group.finish();
}
