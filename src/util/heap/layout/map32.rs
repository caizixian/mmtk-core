use super::map::CreateFreeListResult;
use super::map::VMMap;
use crate::mmtk::SFT_MAP;
use crate::util::conversions;
use crate::util::freelist::FreeList;
use crate::util::heap::layout::heap_parameters::*;
use crate::util::heap::layout::vm_layout::*;
use crate::util::heap::space_descriptor::SpaceDescriptor;
use crate::util::int_array_freelist::IntArrayFreeList;
use crate::util::rust_util::zeroed_alloc::new_zeroed_vec;
use crate::util::Address;
use spin::Mutex;

pub struct Map32 {
    inner: Mutex<Map32Inner>,
}

#[doc(hidden)]
pub struct Map32Inner {
    prev_link: Vec<i32>,
    next_link: Vec<i32>,
    region_map: IntArrayFreeList,
    global_page_map: IntArrayFreeList,
    shared_discontig_fl_count: usize,
    total_available_discontiguous_chunks: usize,
    finalized: bool,
    descriptor_map: Vec<SpaceDescriptor>,
}

unsafe impl Send for Map32 {}
unsafe impl Sync for Map32 {}

impl Map32 {
    pub fn new() -> Self {
        let max_chunks = vm_layout().max_chunks();
        Map32 {
            inner: Mutex::new(Map32Inner {
                prev_link: vec![0; max_chunks],
                next_link: vec![0; max_chunks],
                region_map: IntArrayFreeList::new(max_chunks, max_chunks as _, 1),
                global_page_map: IntArrayFreeList::new(1, 1, MAX_SPACES),
                shared_discontig_fl_count: 0,
                total_available_discontiguous_chunks: 0,
                finalized: false,
                // This can be big on 64-bit machines.  Use `new_zeroed_vec`.
                descriptor_map: new_zeroed_vec(max_chunks),
            }),
        }
    }
}



impl VMMap for Map32 {
    fn insert(&self, start: Address, extent: usize, descriptor: SpaceDescriptor) {
        let mut inner = self.inner.lock();
        let mut e = 0;
        while e < extent {
            let index = (start + e).chunk_index();
            assert!(
                inner.descriptor_map[index].is_empty(),
                "Conflicting virtual address request"
            );
            debug!(
                "Set descriptor {:?} for Chunk {}",
                descriptor,
                conversions::chunk_index_to_address(index)
            );
            inner.descriptor_map[index] = descriptor;
            e += BYTES_IN_CHUNK;
        }
    }

    fn create_freelist(&self, _start: Address) -> CreateFreeListResult {
        let mut inner = self.inner.lock();
        inner.shared_discontig_fl_count += 1;
        let ordinal = inner.shared_discontig_fl_count;
        let free_list = Box::new(IntArrayFreeList::from_parent(
            &inner.global_page_map,
            ordinal as _,
        ));
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
        let mut inner = self.inner.lock();
        let chunk = inner.region_map.alloc(chunks as _);
        debug_assert!(chunk != 0);
        if chunk == -1 {
            return Address::zero();
        }
        inner.total_available_discontiguous_chunks -= chunks;
        let rtn = conversions::chunk_index_to_address(chunk as _);
        
        // Inline insert to avoid deadlock since insert also locks.
        let extent = chunks << LOG_BYTES_IN_CHUNK;
        let mut e = 0;
        while e < extent {
            let index = (rtn + e).chunk_index();
            inner.descriptor_map[index] = descriptor;
            e += BYTES_IN_CHUNK;
        }

        if head.is_zero() {
            debug_assert!(inner.next_link[chunk as usize] == 0);
        } else {
            inner.next_link[chunk as usize] = head.chunk_index() as _;
            inner.prev_link[head.chunk_index()] = chunk;
        }
        debug_assert!(inner.prev_link[chunk as usize] == 0);
        rtn
    }

    fn get_next_contiguous_region(&self, start: Address) -> Address {
        debug_assert!(start == conversions::chunk_align_down(start));
        let inner = self.inner.lock();
        let chunk = start.chunk_index();
        if chunk == 0 || inner.next_link[chunk] == 0 {
            unsafe { Address::zero() }
        } else {
            let a = inner.next_link[chunk];
            conversions::chunk_index_to_address(a as _)
        }
    }

    fn get_contiguous_region_chunks(&self, start: Address) -> usize {
        debug_assert!(start == conversions::chunk_align_down(start));
        let inner = self.inner.lock();
        let chunk = start.chunk_index();
        inner.region_map.size(chunk as i32) as _
    }

    fn get_contiguous_region_size(&self, start: Address) -> usize {
        self.get_contiguous_region_chunks(start) << LOG_BYTES_IN_CHUNK
    }

    fn get_available_discontiguous_chunks(&self) -> usize {
        self.inner.lock().total_available_discontiguous_chunks
    }

    fn get_chunk_consumer_count(&self) -> usize {
        self.inner.lock().shared_discontig_fl_count
    }
    #[allow(clippy::while_immutable_condition)]
    fn free_all_chunks(&self, any_chunk: Address) {
        debug!("free_all_chunks: {}", any_chunk);
        let mut inner = self.inner.lock();
        debug_assert!(any_chunk == conversions::chunk_align_down(any_chunk));
        if !any_chunk.is_zero() {
            let chunk = any_chunk.chunk_index();
            while inner.next_link[chunk] != 0 {
                let x = inner.next_link[chunk];
                Self::free_contiguous_chunks_no_lock(&mut inner, x);
            }
            while inner.prev_link[chunk] != 0 {
                let x = inner.prev_link[chunk];
                Self::free_contiguous_chunks_no_lock(&mut inner, x);
            }
            Self::free_contiguous_chunks_no_lock(&mut inner, chunk as _);
        }
    }

    unsafe fn free_contiguous_chunks(&self, start: Address) -> usize {
        debug!("free_contiguous_chunks: {}", start);
        let mut inner = self.inner.lock();
        debug_assert!(start == conversions::chunk_align_down(start));
        let chunk = start.chunk_index();
        Self::free_contiguous_chunks_no_lock(&mut inner, chunk as _)
    }

    fn finalize_static_space_map(
        &self,
        from: Address,
        to: Address,
        on_discontig_start_determined: &mut dyn FnMut(Address),
    ) {
        let mut inner = self.inner.lock();
        /* establish bounds of discontiguous space */
        let start_address = from;
        let first_chunk = start_address.chunk_index();
        let last_chunk = to.chunk_index();
        let unavail_start_chunk = last_chunk + 1;
        let trailing_chunks = vm_layout().max_chunks() - unavail_start_chunk;
        let pages = (1 + last_chunk - first_chunk) * PAGES_IN_CHUNK;

        inner.global_page_map.resize_freelist(pages, pages as _);

        on_discontig_start_determined(start_address);

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
        inner.region_map.alloc(first_chunk as _); // block out entire bottom of address range
        for _ in first_chunk..=last_chunk {
            inner.region_map.alloc(1);
        }
        let alloced_chunk = inner.region_map.alloc(trailing_chunks as _);
        debug_assert!(
            alloced_chunk == unavail_start_chunk as i32,
            "{} != {}",
            alloced_chunk,
            unavail_start_chunk
        );
        /* set up the global page map and place chunks on free list */
        let mut first_page = 0;
        for chunk_index in first_chunk..=last_chunk {
            inner.total_available_discontiguous_chunks += 1;
            inner.region_map.free(chunk_index as _, false); // put this chunk on the free list
            inner.global_page_map.set_uncoalescable(first_page);
            let alloced_pages = inner.global_page_map.alloc(PAGES_IN_CHUNK as _); // populate the global page map
            debug_assert!(alloced_pages == first_page);
            first_page += PAGES_IN_CHUNK as i32;
        }
        inner.finalized = true;
    }

    fn is_finalized(&self) -> bool {
        self.inner.lock().finalized
    }

    fn get_descriptor_for_address(&self, address: Address) -> SpaceDescriptor {
        let index = address.chunk_index();
        self.inner.lock().descriptor_map
            .get(index)
            .copied()
            .unwrap_or(SpaceDescriptor::UNINITIALIZED)
    }
}

impl Map32 {
    fn free_contiguous_chunks_no_lock(inner: &mut Map32Inner, chunk: i32) -> usize {
        let chunks = inner.region_map.free(chunk, false);
        inner.total_available_discontiguous_chunks += chunks as usize;
        let next = inner.next_link[chunk as usize];
        let prev = inner.prev_link[chunk as usize];
        if next != 0 {
            inner.prev_link[next as usize] = prev
        };
        if prev != 0 {
            inner.next_link[prev as usize] = next
        };
        inner.prev_link[chunk as usize] = 0;
        inner.next_link[chunk as usize] = 0;
        for offset in 0..chunks {
            let index = (chunk + offset) as usize;
            let chunk_start = conversions::chunk_index_to_address(index);
            debug!("Clear descriptor for Chunk {}", chunk_start);
            inner.descriptor_map[index] = SpaceDescriptor::UNINITIALIZED;
            SFT_MAP.clear(chunk_start);
        }
        chunks as _
    }
}

impl Default for Map32 {
    fn default() -> Self {
        Self::new()
    }
}
