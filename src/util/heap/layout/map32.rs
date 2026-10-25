use super::map::CreateFreeListResult;
use super::map::VMMap;
use crate::mmtk::SFT_MAP;
use crate::util::conversions;
use crate::util::freelist::FreeList;
use crate::util::heap::layout::heap_parameters::*;
use crate::util::heap::layout::vm_layout::*;
use crate::util::heap::space_descriptor::{AtomicSpaceDescriptor, SpaceDescriptor};
use crate::util::int_array_freelist::IntArrayFreeList;
use crate::util::rust_util::zeroed_alloc::new_zeroed_vec;
use crate::util::Address;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

struct Map32Sync {
    prev_link: Vec<i32>,
    next_link: Vec<i32>,
    region_map: IntArrayFreeList,
}

pub struct Map32 {
    sync: Mutex<Map32Sync>,
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
            sync: Mutex::new(Map32Sync {
                prev_link: vec![-1; max_chunks],
                next_link: vec![-1; max_chunks],
                region_map: IntArrayFreeList::new(max_chunks, max_chunks as _, 1),
            }),
            global_page_map: OnceLock::new(),
            shared_discontig_fl_count: AtomicUsize::new(0),
            total_available_discontiguous_chunks: AtomicUsize::new(0),
            finalized: AtomicBool::new(false),
            // This can be big on 64-bit machines. Use `new_zeroed_vec`.
            descriptor_map: new_zeroed_vec::<AtomicSpaceDescriptor>(max_chunks).into_boxed_slice(),
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
            //   VM.barriers.objectArrayStoreNoGCBarrier(spaceMap, index, space);
            e += BYTES_IN_CHUNK;
        }
    }

    fn create_freelist(&'static self, _start: Address) -> CreateFreeListResult {
        let global_page_map = self
            .global_page_map
            .get()
            .expect("Map32::create_freelist must be called after finalize_static_space_map initializes global_page_map");
        let ordinal = self
            .shared_discontig_fl_count
            .fetch_add(1, Ordering::Relaxed)
            + 1;
        let free_list = Box::new(IntArrayFreeList::from_parent(global_page_map, ordinal as _));
        CreateFreeListResult {
            free_list,
            space_displacement: 0,
        }
    }

    fn create_parent_freelist(
        &self,
        _start: Address,
        units: usize,
        grain: i32,
    ) -> CreateFreeListResult {
        let free_list = Box::new(IntArrayFreeList::new(units, grain, 1));
        CreateFreeListResult {
            free_list,
            space_displacement: 0,
        }
    }

    unsafe fn allocate_contiguous_chunks(
        &self,
        descriptor: SpaceDescriptor,
        chunks: usize,
        head: Address,
        _maybe_freelist: Option<&mut dyn FreeList>,
    ) -> Address {
        assert!(
            self.finalized.load(Ordering::Acquire),
            "allocate_contiguous_chunks called before finalize_static_space_map"
        );
        let mut sync = self.sync.lock().unwrap();
        let chunk = sync.region_map.alloc(chunks as _);
        debug_assert!(chunk >= -1);
        if chunk == -1 {
            return Address::ZERO;
        }
        self.total_available_discontiguous_chunks
            .fetch_sub(chunks, Ordering::Relaxed);
        let rtn = conversions::chunk_index_to_address(chunk as _);
        self.insert(rtn, chunks << LOG_BYTES_IN_CHUNK, descriptor);
        if head.is_zero() {
            debug_assert_eq!(sync.next_link[chunk as usize], -1);
        } else {
            sync.next_link[chunk as usize] = head.chunk_index() as _;
            sync.prev_link[head.chunk_index()] = chunk;
        }
        debug_assert_eq!(sync.prev_link[chunk as usize], -1);
        rtn
    }

    fn get_next_contiguous_region(&self, start: Address) -> Address {
        if start.is_zero() {
            return Address::ZERO;
        }
        debug_assert!(start == conversions::chunk_align_down(start));
        let chunk = start.chunk_index();
        let sync = self.sync.lock().unwrap();
        let next = sync.next_link[chunk];
        if next == -1 {
            Address::ZERO
        } else {
            conversions::chunk_index_to_address(next as _)
        }
    }

    fn get_contiguous_region_chunks(&self, start: Address) -> usize {
        debug_assert!(start == conversions::chunk_align_down(start));
        let chunk = start.chunk_index();
        let sync = self.sync.lock().unwrap();
        sync.region_map.size(chunk as i32) as _
    }

    fn get_contiguous_region_size(&self, start: Address) -> usize {
        self.get_contiguous_region_chunks(start) << LOG_BYTES_IN_CHUNK
    }

    fn get_available_discontiguous_chunks(&self) -> usize {
        self.total_available_discontiguous_chunks
            .load(Ordering::Relaxed)
    }

    fn get_chunk_consumer_count(&self) -> usize {
        self.shared_discontig_fl_count.load(Ordering::Relaxed)
    }

    fn free_all_chunks(&self, any_chunk: Address) {
        debug!("free_all_chunks: {}", any_chunk);
        let mut sync = self.sync.lock().unwrap();
        debug_assert!(any_chunk == conversions::chunk_align_down(any_chunk));
        if !any_chunk.is_zero() {
            let chunk = any_chunk.chunk_index();
            while sync.next_link[chunk] != -1 {
                let x = sync.next_link[chunk];
                self.free_contiguous_chunks_locked(&mut sync, x);
            }
            while sync.prev_link[chunk] != -1 {
                let x = sync.prev_link[chunk];
                self.free_contiguous_chunks_locked(&mut sync, x);
            }
            self.free_contiguous_chunks_locked(&mut sync, chunk as _);
        }
    }

    unsafe fn free_contiguous_chunks(&self, start: Address) -> usize {
        debug!("free_contiguous_chunks: {}", start);
        let mut sync = self.sync.lock().unwrap();
        debug_assert!(start == conversions::chunk_align_down(start));
        let chunk = start.chunk_index();
        self.free_contiguous_chunks_locked(&mut sync, chunk as _)
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
        // start_address=0xb0000000, first_chunk=704, last_chunk=703, unavail_start_chunk=704, trailing_chunks=320, pages=0
        // startAddress=0x68000000 firstChunk=416 lastChunk=703 unavailStartChunk=704 trailingChunks=320 pages=294912
        {
            let mut sync = self.sync.lock().unwrap();
            assert!(
                !self.finalized.load(Ordering::Relaxed),
                "finalize_static_space_map must only be called once"
            );

            let mut global_page_map = IntArrayFreeList::new(pages, pages as _, MAX_SPACES);

            // [
            //  2: -1073741825
            //  3: -1073741825
            //  5: -2147482624
            //  2048: -2147483648
            //  2049: -2147482624
            //  2050: 1024
            //  2051: 1024
            // ]
            /* set up the region map free list */
            sync.region_map.alloc(first_chunk as _); // block out entire bottom of address range
            for _ in first_chunk..=last_chunk {
                sync.region_map.alloc(1);
            }
            let alloced_chunk = sync.region_map.alloc(trailing_chunks as _);
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
                sync.region_map.free(chunk_index as _, false); // put this chunk on the free list
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

    fn min_contiguous_extent(&self) -> usize {
        BYTES_IN_CHUNK
    }
}

impl Map32 {
    fn free_contiguous_chunks_locked(&self, sync: &mut Map32Sync, chunk: i32) -> usize {
        let chunks = sync.region_map.free(chunk, false);
        debug_assert!(chunks >= 0);
        self.total_available_discontiguous_chunks
            .fetch_add(chunks as usize, Ordering::Relaxed);
        let chunk_idx = chunk as usize;
        let next = sync.next_link[chunk_idx];
        let prev = sync.prev_link[chunk_idx];
        if next != -1 {
            sync.prev_link[next as usize] = prev;
        }
        if prev != -1 {
            sync.next_link[prev as usize] = next;
        }
        sync.prev_link[chunk_idx] = -1;
        sync.next_link[chunk_idx] = -1;
        for offset in 0..chunks {
            let index = (chunk + offset) as usize;
            let chunk_start = conversions::chunk_index_to_address(index);
            debug!("Clear descriptor for Chunk {}", chunk_start);
            self.descriptor_map[index].store(SpaceDescriptor::UNINITIALIZED, Ordering::Relaxed);
            if SFT_MAP.has_sft_entry(chunk_start) {
                // SAFETY: `SFT_MAP.has_sft_entry(chunk_start)` verified that `chunk_start` has a
                // valid, mapped entry in `SFT_MAP`, satisfying `SFTMap::clear`'s precondition.
                unsafe {
                    SFT_MAP.clear(chunk_start);
                }
            }
        }
        chunks as _
    }
}

impl Default for Map32 {
    fn default() -> Self {
        Self::new()
    }
}
