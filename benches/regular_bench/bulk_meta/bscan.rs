//! Benchmarks for scanning side metadata for non-zero bits.

use criterion::Criterion;
use mmtk::util::{
    constants::LOG_BITS_IN_WORD, test_private::scan_non_zero_bits_in_metadata_bytes, Address,
};
use rand::{seq::IteratorRandom, SeedableRng};
use rand_chacha::ChaCha8Rng;

const BLOCK_BYTES: usize = 32768usize; // Match an Immix block size.

// Assume one-bit-per-word metadata (matching VO bits).
const BLOCK_META_BYTES: usize = BLOCK_BYTES >> LOG_BITS_IN_WORD;
const BLOCK_META_WORDS: usize = BLOCK_META_BYTES / std::mem::size_of::<usize>();

/// Set this many distinct bits in the bitmap.
const NUM_OBJECTS: usize = 200;

/// Aligned word-array backing store for the benchmark bitmap (`1024 >= BLOCK_META_BYTES` on both
/// 32-bit and 64-bit targets, matching chunk/block metadata alignment without `alloc_zeroed` leaks).
#[repr(C, align(1024))]
struct AlignedBitmap([usize; BLOCK_META_WORDS]);

/// Get a deterministic seeded Rng.
fn get_rng() -> ChaCha8Rng {
    // Create an Rng from a seed and an explicit Rng type.
    // Not secure at all, but completely deterministic and reproducible.
    // The following seed is read from /dev/random
    const SEED64: u64 = 0x4050cb1b5ab26c70;
    ChaCha8Rng::seed_from_u64(SEED64)
}

/// A bitmap with known locations `(byte_offset, bit_offset)` of each set bit for assertion.
struct PreparedBitmap {
    words: Box<AlignedBitmap>,
    set_bit_offsets: Vec<(usize, u8)>,
}

impl PreparedBitmap {
    fn range(&self) -> (Address, Address) {
        let start = Address::from_ptr(self.words.0.as_ptr());
        (start, start + BLOCK_META_BYTES)
    }

    fn expected_set_bits(&self, start: Address) -> Vec<(Address, u8)> {
        self.set_bit_offsets
            .iter()
            .map(|&(byte_offset, bit)| (start + byte_offset, bit))
            .collect()
    }
}

/// Make a bitmap of the desired size and set bits.
fn make_standard_bitmap() -> PreparedBitmap {
    let mut words = Box::new(AlignedBitmap([0usize; BLOCK_META_WORDS]));
    let mut rng = get_rng();

    let mut set_bit_offsets = (0..(BLOCK_BYTES >> LOG_BITS_IN_WORD))
        .choose_multiple(&mut rng, NUM_OBJECTS)
        .iter()
        .map(|total_bit_offset| {
            let word_offset = total_bit_offset >> LOG_BITS_IN_WORD;
            let bit_in_word = total_bit_offset & ((1 << LOG_BITS_IN_WORD) - 1);
            let byte_offset = (word_offset << LOG_BITS_IN_WORD) + (bit_in_word >> 3);
            (byte_offset, (bit_in_word & 7) as u8)
        })
        .collect::<Vec<_>>();

    set_bit_offsets.sort();

    let bytes: &mut [u8] = bytemuck::cast_slice_mut(&mut words.0);
    for &(byte_offset, bit) in &set_bit_offsets {
        bytes[byte_offset] |= 1u8 << bit;
    }

    PreparedBitmap {
        words,
        set_bit_offsets,
    }
}

pub fn bench(c: &mut Criterion) {
    c.bench_function("bscan_block", |b| {
        let bitmap = make_standard_bitmap();
        let (start, end) = bitmap.range();
        let expected_set_bits = bitmap.expected_set_bits(start);
        let mut holder: Vec<(Address, u8)> = Vec::with_capacity(NUM_OBJECTS);

        b.iter(|| {
            holder.clear();
            // SAFETY: `[start, end)` is backed by `bitmap.words` (`Box<AlignedBitmap>`, non-null,
            // mapped, and initialized), exclusively owned by this single-threaded benchmark, and
            // the visitor closure only pushes to `holder` without mutating `bitmap.words`.
            unsafe {
                scan_non_zero_bits_in_metadata_bytes(start, end, &mut |addr, shift| {
                    holder.push((addr, shift));
                });
            }
        });

        assert_eq!(holder.len(), NUM_OBJECTS);
        assert_eq!(holder, expected_set_bits);
    });
}
