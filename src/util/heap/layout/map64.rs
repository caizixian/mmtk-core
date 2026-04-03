use super::map::CreateFreeListResult;
use super::map::VMMap;
use crate::util::constants::*;
use crate::util::conversions;
use crate::util::freelist::FreeList;
use crate::util::heap::layout::heap_parameters::*;
use crate::util::heap::layout::vm_layout::*;
use crate::util::heap::space_descriptor::SpaceDescriptor;
use crate::util::memory::MmapStrategy;
use crate::util::raw_memory_freelist::RawMemoryFreeList;
use crate::util::Address;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const NON_MAP_FRACTION: f64 = 1.0 - 8.0 / 4096.0;

pub struct Map64 {
    finalized: AtomicBool,
    descriptor_map: Vec<AtomicUsize>,
    base_address: Vec<AtomicUsize>,
    high_water: Vec<AtomicUsize>,
}

impl Map64 {
    pub fn new() -> Self {
        let descriptor_map = (0..MAX_SPACES)
            .map(|_| AtomicUsize::new(SpaceDescriptor::UNINITIALIZED.as_usize()))
            .collect();
        let base_address = (0..MAX_SPACES)
            .map(|i| AtomicUsize::new(i << vm_layout().log_space_extent))
            .collect();
        let high_water = (0..MAX_SPACES)
            .map(|i| AtomicUsize::new(i << vm_layout().log_space_extent))
            .collect();

        Self {
            descriptor_map,
            base_address,
            high_water,
            finalized: AtomicBool::new(false),
        }
    }
}

impl VMMap for Map64 {
    fn insert(&self, start: Address, extent: usize, descriptor: SpaceDescriptor) {
        debug_assert!(Self::is_space_start(start));
        debug_assert!(extent <= vm_layout().space_size_64());
        let index = Self::space_index(start).unwrap();
        self.descriptor_map[index].store(descriptor.as_usize(), Ordering::Relaxed);
    }

    fn create_freelist(&self, start: Address) -> CreateFreeListResult {
        let units = vm_layout().space_size_64() >> LOG_BYTES_IN_PAGE;
        self.create_parent_freelist(start, units, units as _)
    }

    fn create_parent_freelist(
        &self,
        start: Address,
        mut units: usize,
        grain: i32,
    ) -> CreateFreeListResult {
        debug_assert!(start.is_aligned_to(BYTES_IN_CHUNK));

        let index = Self::space_index(start).unwrap();

        units = (units as f64 * NON_MAP_FRACTION) as _;
        let list_extent =
            conversions::pages_to_bytes(RawMemoryFreeList::size_in_pages(units as _, 1) as _);

        let heads = 1;
        let pages_per_block = RawMemoryFreeList::default_block_size(units as _, heads);
        let list = Box::new(RawMemoryFreeList::new(
            start,
            start + list_extent,
            pages_per_block,
            units as _,
            grain,
            heads,
            MmapStrategy::INTERNAL_MEMORY,
        ));

        /* Adjust the base address and highwater to account for the allocated chunks for the map */
        let base = conversions::chunk_align_up(start + list_extent);

        self.high_water[index].store(base.as_usize(), Ordering::Relaxed);
        self.base_address[index].store(base.as_usize(), Ordering::Relaxed);

        let space_displacement = base - start;
        CreateFreeListResult {
            free_list: list,
            space_displacement,
        }
    }

    unsafe fn allocate_contiguous_chunks(
        &self,
        descriptor: SpaceDescriptor,
        chunks: usize,
        _head: Address,
        maybe_freelist: Option<&mut dyn FreeList>,
    ) -> Address {
        debug_assert!(Self::space_index(descriptor.get_start()).unwrap() == descriptor.get_index());

        let index = descriptor.get_index();
        let extent = chunks << LOG_BYTES_IN_CHUNK;
        
        let rtn_usize = self.high_water[index].fetch_add(extent, Ordering::Relaxed);
        let rtn = unsafe { Address::from_usize(rtn_usize) };

        if let Some(freelist) = maybe_freelist {
            let Some(rmfl) = freelist.downcast_mut::<RawMemoryFreeList>() else {
                panic!("Map64 requires a growable free list implementation (RawMemoryFreeList).");
            };
            rmfl.grow_freelist(conversions::bytes_to_pages_up(extent) as _);
            let base_addr_usize = self.base_address[index].load(Ordering::Relaxed);
            let base_addr = unsafe { Address::from_usize(base_addr_usize) };
            let base_page = conversions::bytes_to_pages_up(rtn - base_addr);
            for offset in (0..(chunks * PAGES_IN_CHUNK)).step_by(PAGES_IN_CHUNK) {
                rmfl.set_uncoalescable((base_page + offset) as _);
                rmfl.alloc_from_unit(PAGES_IN_CHUNK as _, (base_page + offset) as _);
            }
        }
        rtn
    }

    fn get_next_contiguous_region(&self, _start: Address) -> Address {
        unreachable!()
    }

    fn get_contiguous_region_chunks(&self, _start: Address) -> usize {
        unreachable!()
    }

    fn get_contiguous_region_size(&self, _start: Address) -> usize {
        unreachable!()
    }

    fn get_available_discontiguous_chunks(&self) -> usize {
        panic!("We don't use discontiguous chunks for 64-bit!");
    }

    fn get_chunk_consumer_count(&self) -> usize {
        panic!("We don't use discontiguous chunks for 64-bit!");
    }

    fn free_all_chunks(&self, _any_chunk: Address) {
        unreachable!()
    }

    unsafe fn free_contiguous_chunks(&self, _start: Address) -> usize {
        unreachable!()
    }

    fn finalize_static_space_map(
        &self,
        _from: Address,
        _to: Address,
        _on_discontig_start_determined: &mut dyn FnMut(Address),
    ) {
        self.finalized.store(true, Ordering::Relaxed);
    }

    fn is_finalized(&self) -> bool {
        self.finalized.load(Ordering::Relaxed)
    }

    fn get_descriptor_for_address(&self, address: Address) -> SpaceDescriptor {
        if let Some(index) = Self::space_index(address) {
            let val = self.descriptor_map[index].load(Ordering::Relaxed);
            SpaceDescriptor::from_usize(val)
        } else {
            SpaceDescriptor::UNINITIALIZED
        }
    }
}

impl Map64 {
    fn space_index(addr: Address) -> Option<usize> {
        if addr > vm_layout().heap_end {
            return None;
        }
        Some(addr >> vm_layout().space_shift_64())
    }

    fn is_space_start(base: Address) -> bool {
        (base & !vm_layout().space_mask_64()) == 0
    }
}

impl Default for Map64 {
    fn default() -> Self {
        Self::new()
    }
}
