use super::map::CreateFreeListResult;
use super::map::VMMap;
use crate::util::constants::*;
use crate::util::conversions;
use crate::util::freelist::FreeList;
use crate::util::heap::layout::heap_parameters::*;
use crate::util::heap::layout::vm_layout::*;
use crate::util::heap::space_descriptor::{AtomicSpaceDescriptor, SpaceDescriptor};
use crate::util::os::*;
use crate::util::raw_memory_freelist::RawMemoryFreeList;
use crate::util::Address;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

const NON_MAP_FRACTION: f64 = 1.0 - 8.0 / 4096.0;

#[derive(Clone, Copy)]
struct SpaceState {
    base_address: Address,
    high_water: Address,
}

pub struct Map64 {
    finalized: AtomicBool,
    descriptor_map: [AtomicSpaceDescriptor; MAX_SPACES],
    space_state: [Mutex<SpaceState>; MAX_SPACES],
}

assert_impl_all!(Map64: Send, Sync);

impl Map64 {
    pub fn new() -> Self {
        let log_space_extent = vm_layout().log_space_extent;
        let space_state = std::array::from_fn(|i| {
            // SAFETY: `i < MAX_SPACES` (`16`) and `vm_layout().validate()` ensures that in 64-bit
            // contiguous layouts (`force_use_contiguous_spaces == true`),
            // `log_space_extent <= log_address_space - LOG_MAX_SPACES < usize::BITS`, so
            // `i << log_space_extent` is a non-overflowing virtual base address boundary.
            // `Address::from_usize` only wraps the raw integer address without dereferencing it.
            let base = unsafe { Address::from_usize(i << log_space_extent) };
            Mutex::new(SpaceState {
                base_address: base,
                high_water: base,
            })
        });
        let descriptor_map =
            std::array::from_fn(|_| AtomicSpaceDescriptor::new(SpaceDescriptor::UNINITIALIZED));

        Self {
            finalized: AtomicBool::new(false),
            descriptor_map,
            space_state,
        }
    }
}

impl VMMap for Map64 {
    fn insert(&self, start: Address, extent: usize, descriptor: SpaceDescriptor) {
        debug_assert!(Self::is_space_start(start));
        debug_assert!(extent <= vm_layout().space_size_64());
        debug_assert!(extent >= self.min_contiguous_extent());
        let index = Self::space_index(start).unwrap();
        self.descriptor_map[index].store(descriptor, Ordering::Relaxed);
    }

    fn create_freelist(&'static self, start: Address) -> CreateFreeListResult {
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

        assert!(units >= conversions::bytes_to_pages_up(self.min_contiguous_extent()));

        let index = Self::space_index(start).unwrap();
        let mut state = self.space_state[index].lock().unwrap();

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
            MmapStrategy::RAW_MEMORY_FREELIST,
        ));

        /* Adjust the base address and highwater to account for the allocated chunks for the map */
        let base = conversions::chunk_align_up(start + list_extent);

        state.high_water = base;
        state.base_address = base;

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
        debug_assert!(Self::space_index(descriptor.get_start()) == Some(descriptor.get_index()));

        let index = descriptor.get_index();
        let mut state = self.space_state[index].lock().unwrap();
        let rtn = state.high_water;
        let extent = chunks << LOG_BYTES_IN_CHUNK;
        state.high_water = rtn + extent;
        let base_address = state.base_address;

        if let Some(freelist) = maybe_freelist {
            let Some(rmfl) = freelist.downcast_mut::<RawMemoryFreeList>() else {
                // `Map64` allocates chunks by raising the high water mark to provide previously
                // uncovered address range to the caller.  Therefore if the `PageResource` that
                // made the allocation request is based on freelist, the freelist must be grown to
                // accommodate the new chunks.  Currently only `RawMemoryFreeList` can grow.
                panic!("Map64 requires a growable free list implementation (RawMemoryFreeList).");
            };
            rmfl.grow_freelist(conversions::bytes_to_pages_up(extent) as _);
            let base_page = conversions::bytes_to_pages_up(rtn - base_address);
            for offset in (0..(chunks * PAGES_IN_CHUNK)).step_by(PAGES_IN_CHUNK) {
                rmfl.set_uncoalescable((base_page + offset) as _);
                /* The 32-bit implementation requires that pages are returned allocated to the caller */
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
        // Note: When using Map64, the starting address of each space is adjusted as soon as the
        // `RawMemoryFreeList` instance in its underlying `FreeListPageResource` is created.  We no
        // longer need to adjust the starting address here.  So we ignore the
        // `_on_discontig_start_determined` callback which may adjust the starting address.
        self.finalized.store(true, Ordering::Release);
    }

    fn is_finalized(&self) -> bool {
        self.finalized.load(Ordering::Acquire)
    }

    fn get_descriptor_for_address(&self, address: Address) -> SpaceDescriptor {
        if let Some(index) = Self::space_index(address) {
            self.descriptor_map
                .get(index)
                .map(|slot| slot.load(Ordering::Relaxed))
                .unwrap_or(SpaceDescriptor::UNINITIALIZED)
        } else {
            SpaceDescriptor::UNINITIALIZED
        }
    }

    fn min_contiguous_extent(&self) -> usize {
        // The underlying RawMemoryFreeList will occupy a portion of address range as its own metadata from
        // the space extent. As we always allocate in the granularity of chunks, for one chunk of space,
        // we would need a small fraction as the metadata. So make the minimal space size allowed to be two chunks.
        BYTES_IN_CHUNK * 2
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
