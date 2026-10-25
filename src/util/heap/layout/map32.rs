use super::map::{AllocatedChunkRegion, VMMap};
use crate::mmtk::SFT_MAP;
use crate::util::conversions;
use crate::util::freelist::FreeList;
use crate::util::heap::layout::heap_parameters::*;
use crate::util::heap::layout::vm_layout::*;
use crate::util::heap::space_descriptor::{AtomicSpaceDescriptor, SpaceDescriptor};
use crate::util::int_array_freelist::IntArrayFreeList;
use crate::util::Address;
use bytemuck::allocation::zeroed_slice_box;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

pub struct Map32 {
    region_map: Mutex<IntArrayFreeList>,
    /// Shared parent page freelist initialized once in [`Map32::finalize_static_space_map`] before
    /// any child [`IntArrayFreeList`] instances are created via [`Map32::create_freelist`].
    global_page_map: OnceLock<IntArrayFreeList>,
    shared_discontig_fl_count: AtomicUsize,
    total_available_discontiguous_chunks: AtomicUsize,
    finalized: AtomicBool,
    descriptor_map: Box<[AtomicSpaceDescriptor]>,
}

assert_impl_all!(Map32: Send, Sync);

impl Map32 {
    pub fn new() -> Self {
        let max_chunks = vm_layout().max_chunks();
        Map32 {
            region_map: Mutex::new(IntArrayFreeList::new(max_chunks, max_chunks as _, 1)),
            global_page_map: OnceLock::new(),
            shared_discontig_fl_count: AtomicUsize::new(0),
            total_available_discontiguous_chunks: AtomicUsize::new(0),
            finalized: AtomicBool::new(false),
            // This can be big on 64-bit machines; allocate zeroed pages directly via `alloc_zeroed`.
            descriptor_map: zeroed_slice_box::<AtomicSpaceDescriptor>(max_chunks),
        }
    }
}

impl VMMap for Map32 {
    fn insert(&self, start: Address, extent: usize, descriptor: SpaceDescriptor) {
        let mut e = 0;
        while e < extent {
            let index = (start + e).chunk_index();
            self.descriptor_map[index]
                .compare_exchange(
                    SpaceDescriptor::UNINITIALIZED,
                    descriptor,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .unwrap_or_else(|existing| {
                    panic!(
                        "Conflicting virtual address request at chunk {} (existing {:?})",
                        conversions::chunk_index_to_address(index),
                        existing
                    )
                });
            debug!(
                "Set descriptor {:?} for Chunk {}",
                descriptor,
                conversions::chunk_index_to_address(index)
            );
            e += BYTES_IN_CHUNK;
        }
    }

    fn create_freelist(&'static self, _start: Address) -> Box<dyn FreeList> {
        let global_page_map = self
            .global_page_map
            .get()
            .expect("Map32::create_freelist must be called after finalize_static_space_map initializes global_page_map");
        let ordinal = self
            .shared_discontig_fl_count
            .fetch_add(1, Ordering::Relaxed)
            + 1;
        Box::new(IntArrayFreeList::from_parent(global_page_map, ordinal as _))
    }

    fn create_parent_freelist(
        &self,
        _start: Address,
        units: usize,
        grain: i32,
    ) -> Box<dyn FreeList> {
        Box::new(IntArrayFreeList::new(units, grain, 1))
    }

    fn allocate_contiguous_chunks(
        &self,
        descriptor: SpaceDescriptor,
        chunks: usize,
        _maybe_freelist: Option<&mut dyn FreeList>,
    ) -> Option<AllocatedChunkRegion> {
        assert!(
            self.finalized.load(Ordering::Acquire),
            "allocate_contiguous_chunks called before finalize_static_space_map"
        );
        if chunks == 0 {
            return None;
        }
        let chunk = self.region_map.lock().unwrap().alloc(chunks as _);
        debug_assert!(chunk >= -1);
        if chunk == -1 {
            return None;
        }
        self.total_available_discontiguous_chunks
            .fetch_sub(chunks, Ordering::Relaxed);
        let rtn = conversions::chunk_index_to_address(chunk as _);
        self.insert(rtn, chunks << LOG_BYTES_IN_CHUNK, descriptor);
        Some(AllocatedChunkRegion::new(rtn, chunks))
    }

    fn free_contiguous_chunks(&self, region: AllocatedChunkRegion) -> usize {
        let start = region.start();
        debug!("free_contiguous_chunks: {}", start);
        assert!(
            self.finalized.load(Ordering::Acquire),
            "free_contiguous_chunks called before finalize_static_space_map"
        );
        let chunk = start.chunk_index();
        assert!(
            chunk < self.descriptor_map.len(),
            "chunk index {chunk} out of bounds"
        );
        let chunks = {
            let mut region_map = self.region_map.lock().unwrap();
            assert!(
                !region_map.get_free(chunk as i32),
                "attempt to free unallocated chunk {chunk}"
            );
            let freed = region_map.free(chunk as i32, false);
            assert_eq!(
                freed as usize,
                region.chunks(),
                "freed chunk count ({freed}) does not match AllocatedChunkRegion ({})",
                region.chunks()
            );
            freed as usize
        };
        self.total_available_discontiguous_chunks
            .fetch_add(chunks, Ordering::Relaxed);
        for offset in 0..chunks {
            let index = chunk + offset;
            let chunk_start = conversions::chunk_index_to_address(index);
            debug!("Clear descriptor for Chunk {}", chunk_start);
            self.descriptor_map[index].store(SpaceDescriptor::UNINITIALIZED, Ordering::Relaxed);
            SFT_MAP.clear_checked(chunk_start);
        }
        chunks
    }

    fn get_available_discontiguous_chunks(&self) -> usize {
        self.total_available_discontiguous_chunks
            .load(Ordering::Relaxed)
    }

    fn get_chunk_consumer_count(&self) -> usize {
        self.shared_discontig_fl_count.load(Ordering::Relaxed)
    }

    fn finalize_static_space_map(
        &self,
        from: Address,
        to: Address,
        on_discontig_start_determined: &mut dyn FnMut(Address),
    ) {
        /* establish bounds of discontiguous space */
        let start_address = from;
        let first_chunk = start_address.chunk_index();
        let last_chunk = to.chunk_index();
        let unavail_start_chunk = last_chunk + 1;
        let trailing_chunks = vm_layout().max_chunks() - unavail_start_chunk;
        let pages = (1 + last_chunk - first_chunk) * PAGES_IN_CHUNK;
        {
            let mut region_map = self.region_map.lock().unwrap();
            assert!(
                !self.finalized.load(Ordering::Relaxed),
                "finalize_static_space_map must only be called once"
            );

            let mut global_page_map = IntArrayFreeList::new(pages, pages as _, MAX_SPACES);

            /* set up the region map free list */
            region_map.alloc(first_chunk as _); // block out entire bottom of address range
            for _ in first_chunk..=last_chunk {
                region_map.alloc(1);
            }
            let alloced_chunk = region_map.alloc(trailing_chunks as _);
            debug_assert!(
                alloced_chunk == unavail_start_chunk as i32,
                "{} != {}",
                alloced_chunk,
                unavail_start_chunk
            );
            /* set up the global page map and place chunks on free list */
            let mut avail_chunks = 0;
            let mut first_page = 0;
            for chunk_index in first_chunk..=last_chunk {
                avail_chunks += 1;
                region_map.free(chunk_index as _, false); // put this chunk on the free list
                global_page_map.set_uncoalescable(first_page);
                let alloced_pages = global_page_map.alloc(PAGES_IN_CHUNK as _); // populate the global page map
                debug_assert!(alloced_pages == first_page);
                first_page += PAGES_IN_CHUNK as i32;
            }
            self.global_page_map
                .set(global_page_map)
                .expect("finalize_static_space_map must only initialize global_page_map once");
            self.total_available_discontiguous_chunks
                .store(avail_chunks, Ordering::Relaxed);
            self.finalized.store(true, Ordering::Release);
        }

        on_discontig_start_determined(start_address);
    }

    fn is_finalized(&self) -> bool {
        self.finalized.load(Ordering::Acquire)
    }

    fn get_descriptor_for_address(&self, address: Address) -> SpaceDescriptor {
        let index = address.chunk_index();
        self.descriptor_map
            .get(index)
            .map(|slot| slot.load(Ordering::Relaxed))
            .unwrap_or(SpaceDescriptor::UNINITIALIZED)
    }
}

impl Default for Map32 {
    fn default() -> Self {
        Self::new()
    }
}
