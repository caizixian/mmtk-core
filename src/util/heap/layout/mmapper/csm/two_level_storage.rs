//! This module contains [`TwoLevelStateStorage`], an implementation of [`MapStateStorage`] that is
//! designed to work well on 64-bit machines.  Currently it supports 48-bit address spaces, and many
//! constants and data structures (such as [`Slab`]) are larger than `i32::MAX`.  For this reason,
//! this module is only available on 64-bit machines.
use super::MapState;
use crate::util::conversions;
use crate::util::heap::layout::mmapper::csm::{ChunkRange, MapStateStorage};
use crate::util::heap::layout::vm_layout::*;
use crate::util::os::MmapResult;
use crate::util::rust_util::rev_group::RevisitableGroupByForIterator;
use crate::util::Address;
use atomic::{Atomic, Ordering};
use std::fmt;
use std::sync::OnceLock;

/// Logarithm of the address space size that [`TwoLevelStateStorage`] is able to handle.
/// This is enough for ARM64, x86_64 and some other architectures.
/// Feel free to increase it if we plan to support larger address spaces.
const LOG_MAPPABLE_BYTES: usize = 48;
/// Total number of chunks covered by [`LOG_MAPPABLE_BYTES`].
const MAPPABLE_CHUNKS: usize = 1 << (LOG_MAPPABLE_BYTES - LOG_BYTES_IN_CHUNK);

/// Log number of bytes per slab. For a two-level array, it is advisable to choose the arithmetic
/// mean of [`LOG_MAPPABLE_BYTES`] and [`LOG_BYTES_IN_CHUNK`] in order to balance the array size
/// of [`TwoLevelStateStorage::slabs`] and [`Slab`].
///
/// TODO: Use `usize::midpoint` after bumping MSRV to 1.85
const LOG_MMAP_SLAB_BYTES: usize =
    LOG_BYTES_IN_CHUNK + (LOG_MAPPABLE_BYTES - LOG_BYTES_IN_CHUNK) / 2;

/// Log number of chunks per slab.
const LOG_MMAP_CHUNKS_PER_SLAB: usize = LOG_MMAP_SLAB_BYTES - LOG_BYTES_IN_CHUNK;
/// Number of chunks per slab.
const MMAP_CHUNKS_PER_SLAB: usize = 1 << LOG_MMAP_CHUNKS_PER_SLAB;

/// Logarithm of maximum number of slabs.
const LOG_MAX_SLABS: usize = LOG_MAPPABLE_BYTES - LOG_MMAP_SLAB_BYTES;
/// maximum number of slabs.
const MAX_SLABS: usize = 1 << LOG_MAX_SLABS;

/// The slab type.  Each slab holds the `MapState` of multiple chunks.
type Slab = [Atomic<MapState>; MMAP_CHUNKS_PER_SLAB];

/// A two-level implementation of `MapStateStorage`.
///
/// It is essentially a lazily initialized array of [`Atomic<MapState>`].  Because it is designed to
/// govern a large address range, and the array is sparse, we use a two-level design.  The higher
/// level holds a boxed slice of [`OnceLock<Box<Slab>>`], and each slab holds an array of
/// [`Atomic<MapState>`].  Each slab governs an aligned region of [`MMAP_CHUNKS_PER_SLAB`] chunks.
/// Slabs are lazily created when the user intends to write into one of its `MapState`.
pub struct TwoLevelStateStorage {
    /// Lazily initialized slabs.
    slabs: Box<[OnceLock<Box<Slab>>]>,
}

assert_impl_all!(Slab: Send, Sync);
assert_impl_all!(TwoLevelStateStorage: Send, Sync);

impl fmt::Debug for TwoLevelStateStorage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TwoLevelMapper({})", 1 << LOG_MAX_SLABS)
    }
}

impl MapStateStorage for TwoLevelStateStorage {
    fn log_mappable_bytes(&self) -> u8 {
        LOG_MAPPABLE_BYTES as u8
    }

    fn get_state(&self, chunk: Address) -> MapState {
        let chunk_idx = chunk.chunk_index();
        let Some(slab) = self.slab_table(chunk_idx) else {
            return MapState::Unmapped;
        };
        slab[Self::in_slab_index(chunk_idx)].load(Ordering::Relaxed)
    }

    fn bulk_set_state(&self, range: ChunkRange, state: MapState) {
        if range.is_empty() {
            return;
        }

        if range.is_single_chunk() {
            let chunk_idx = range.start_chunk;
            let slab = self.get_or_allocate_slab_table(chunk_idx);
            slab[Self::in_slab_index(chunk_idx)].store(state, Ordering::Relaxed);
            return;
        }

        self.foreach_slab_slice_for_write(range, |slice| {
            for slot in slice {
                slot.store(state, Ordering::Relaxed);
            }
        });
    }

    fn bulk_transition_state<F>(&self, range: ChunkRange, mut update_fn: F) -> MmapResult<()>
    where
        F: FnMut(ChunkRange, MapState) -> MmapResult<Option<MapState>>,
    {
        if range.is_empty() {
            return Ok(());
        }

        if range.is_single_chunk() {
            let chunk_idx = range.start_chunk;
            let slab = self.get_or_allocate_slab_table(chunk_idx);
            let slot = &slab[Self::in_slab_index(chunk_idx)];

            let old_state = slot.load(Ordering::Relaxed);
            if let Some(new_state) = update_fn(range, old_state)? {
                slot.store(new_state, Ordering::Relaxed);
            };

            return Ok(());
        }

        let mut slice_indices = Vec::new();

        self.foreach_slab_slice_for_write(range, |slice| {
            slice_indices.push(slice);
        });

        let mut group_start = range.start_chunk;

        for group in slice_indices
            .iter()
            .copied()
            .flatten()
            .revisitable_group_by(|s| s.load(Ordering::Relaxed))
        {
            let state = group.key;
            let group_len = group.len;
            let group_range = ChunkRange::new(group_start, group_len);

            if let Some(new_state) = update_fn(group_range, state)? {
                for slot in group {
                    slot.store(new_state, Ordering::Relaxed);
                }
            }

            group_start += group_len;
        }

        Ok(())
    }
}

impl TwoLevelStateStorage {
    pub fn new() -> Self {
        let slabs: Vec<OnceLock<Box<Slab>>> = std::iter::repeat_with(OnceLock::new)
            .take(MAX_SLABS)
            .collect();
        Self {
            slabs: slabs.into_boxed_slice(),
        }
    }

    fn new_slab() -> Slab {
        std::array::from_fn(|_| Atomic::new(MapState::Unmapped))
    }

    fn slab_table(&self, chunk_idx: usize) -> Option<&Slab> {
        self.slabs
            .get(Self::slab_index(chunk_idx))?
            .get()
            .map(Box::as_ref)
    }

    fn get_or_allocate_slab_table(&self, chunk_idx: usize) -> &Slab {
        let index = Self::slab_index(chunk_idx);
        let Some(slot) = self.slabs.get(index) else {
            panic!(
                "Cannot allocate slab for chunk {chunk_idx} (address {})",
                conversions::chunk_index_to_address(chunk_idx)
            );
        };
        slot.get_or_init(|| Box::new(Self::new_slab()))
    }

    fn slab_index(chunk_idx: usize) -> usize {
        chunk_idx >> LOG_MMAP_CHUNKS_PER_SLAB
    }

    fn in_slab_index(chunk_idx: usize) -> usize {
        chunk_idx & (MMAP_CHUNKS_PER_SLAB - 1)
    }

    /// Visit all slabs that overlap with the `range` from low to high chunk index.  For each slab,
    /// call `f` with the slice of the slab that overlaps with the chunks within the `range`.
    fn foreach_slab_slice_for_write<'s, F>(&'s self, range: ChunkRange, mut f: F)
    where
        F: FnMut(&'s [Atomic<MapState>]),
    {
        debug_assert!(
            range.is_within_limit(MAPPABLE_CHUNKS),
            "range {range} out of bound"
        );

        let limit = range.end_chunk();
        let mut low = range.start_chunk;
        while low < limit {
            let next_slab_boundary =
                ((low >> LOG_MMAP_CHUNKS_PER_SLAB) + 1) << LOG_MMAP_CHUNKS_PER_SLAB;
            let high = next_slab_boundary.min(limit);

            let slab = self.get_or_allocate_slab_table(low);
            let low_index = Self::in_slab_index(low);
            let ub_index = low_index + (high - low);
            f(&slab[low_index..ub_index]);

            low = high;
        }
    }
}

impl Default for TwoLevelStateStorage {
    fn default() -> Self {
        Self::new()
    }
}
