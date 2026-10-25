use super::MapState;
use crate::util::heap::layout::mmapper::csm::ChunkRange;
use crate::util::heap::layout::mmapper::csm::MapStateStorage;
use crate::util::rust_util::rev_group::RevisitableGroupByForIterator;
use crate::util::Address;

use crate::util::heap::layout::vm_layout::*;
use std::fmt;
use std::sync::atomic::Ordering;

use crate::util::os::MmapResult;
use atomic::Atomic;

/// Logarithm of the address space size that [`ByteMapStateStorage`] is able to handle.
/// This is enough for 32-bit architectures.
/// We may increase it beyond 32 so that it is usable on 64-bit machines in certain VMs with
/// limited address spaces, too.
const LOG_MAPPABLE_BYTES: usize = 32;

/// For now, we only use `ByteMapStateStorage` for 32-bit address range.
const MMAP_NUM_CHUNKS: usize = 1 << (LOG_MAPPABLE_BYTES - LOG_BYTES_IN_CHUNK);

/// A [`MapStateStorage`] implementation based on a simple array.
///
/// Currently it is sized to cover a 32-bit address range.
pub struct ByteMapStateStorage {
    mapped: [Atomic<MapState>; MMAP_NUM_CHUNKS],
}

impl fmt::Debug for ByteMapStateStorage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ByteMapStateStorage({})", MMAP_NUM_CHUNKS)
    }
}

impl MapStateStorage for ByteMapStateStorage {
    fn log_mappable_bytes(&self) -> u8 {
        LOG_MAPPABLE_BYTES as u8
    }

    fn get_state(&self, chunk: Address) -> MapState {
        let index = chunk.chunk_index();
        let Some(slot) = self.mapped.get(index) else {
            return MapState::Unmapped;
        };
        slot.load(Ordering::Relaxed)
    }

    fn bulk_set_state(&self, range: ChunkRange, state: MapState) {
        for slot in &self.mapped[range.start_chunk..range.end_chunk()] {
            slot.store(state, Ordering::Relaxed);
        }
    }

    fn bulk_transition_state<F>(&self, range: ChunkRange, mut update_fn: F) -> MmapResult<()>
    where
        F: FnMut(ChunkRange, MapState) -> MmapResult<Option<MapState>>,
    {
        if range.is_empty() {
            return Ok(());
        }

        if range.is_single_chunk() {
            let slot: &Atomic<MapState> = &self.mapped[range.start_chunk];
            let state = slot.load(Ordering::Relaxed);
            if let Some(new_state) = update_fn(range, state)? {
                slot.store(new_state, Ordering::Relaxed);
            }
            return Ok(());
        }

        let mut group_start = range.start_chunk;
        for group in self.mapped.as_slice()[range.start_chunk..range.end_chunk()]
            .iter()
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

impl ByteMapStateStorage {
    pub fn new() -> Self {
        ByteMapStateStorage {
            mapped: [const { Atomic::new(MapState::Unmapped) }; MMAP_NUM_CHUNKS],
        }
    }
}

impl Default for ByteMapStateStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::conversions;

    #[test]
    fn top_chunk_32bit_and_arch_boundary_do_not_overflow() {
        let storage = ByteMapStateStorage::new();
        let mappable_chunks =
            1usize << (storage.log_mappable_bytes() as usize - LOG_BYTES_IN_CHUNK);
        assert_eq!(mappable_chunks, 1024);

        // Chunk 1023 (`[0xffc0_0000, 2^32)`) previously overflowed 32-bit `Address + usize`.
        let chunk_1023 = conversions::chunk_index_to_address(1023);
        let aligned = ChunkRange::new_aligned(chunk_1023, BYTES_IN_CHUNK);
        assert_eq!(aligned, ChunkRange::new(1023, 1));
        assert!(aligned.is_within_limit(mappable_chunks));
        assert_eq!(aligned.start_address(), chunk_1023);
        assert_eq!(aligned.bytes(), BYTES_IN_CHUNK);

        let unaligned = ChunkRange::new_unaligned(chunk_1023 + 4096usize, BYTES_IN_CHUNK - 4096);
        assert_eq!(unaligned, ChunkRange::new(1023, 1));

        // Transition chunks 1022..=1023 (`0xff80_0000..2^32`) across multiple states.
        let chunk_1022 = conversions::chunk_index_to_address(1022);
        let span = ChunkRange::new_unaligned(chunk_1022 + 4096usize, 2 * BYTES_IN_CHUNK - 4096);
        assert_eq!(span, ChunkRange::new(1022, 2));
        storage
            .bulk_transition_state(span, |group_range, state| {
                assert_eq!(state, MapState::Unmapped);
                assert_eq!(group_range, ChunkRange::new(1022, 2));
                Ok(Some(MapState::Quarantined))
            })
            .unwrap();
        assert_eq!(storage.get_state(chunk_1022), MapState::Quarantined);
        assert_eq!(storage.get_state(chunk_1023), MapState::Quarantined);

        storage.bulk_set_state(aligned, MapState::Mapped);
        assert_eq!(storage.get_state(chunk_1022), MapState::Quarantined);
        assert_eq!(storage.get_state(chunk_1023), MapState::Mapped);

        // Verify the top chunk of the host architecture (`[usize::MAX - BYTES_IN_CHUNK + 1, usize::MAX]`)
        // also constructs `ChunkRange` without `Address` addition overflow.
        let arch_top_chunk_start = Address::MAX - (BYTES_IN_CHUNK - 1);
        let arch_top = ChunkRange::new_unaligned(arch_top_chunk_start, BYTES_IN_CHUNK);
        assert!(arch_top.is_single_chunk());
        assert_eq!(arch_top.start_address(), arch_top_chunk_start);
        assert_eq!(arch_top.bytes(), BYTES_IN_CHUNK);
    }
}
