use super::map::{AllocatedChunkRegion, VMMap};
use crate::util::constants::*;
use crate::util::conversions;
use crate::util::freelist::{self, FreeList};
use crate::util::heap::layout::heap_parameters::*;
use crate::util::heap::layout::vm_layout::*;
use crate::util::heap::space_descriptor::{AtomicSpaceDescriptor, SpaceDescriptor};
use crate::util::os::*;
use crate::util::raw_memory_freelist::RawMemoryFreeList;
use crate::util::Address;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

#[derive(Clone, Copy)]
struct SpaceState {
    base_address: Address,
    high_water: Address,
    freelist_created: bool,
}

pub struct Map64 {
    heap_end: Address,
    log_space_extent: usize,
    finalized: AtomicBool,
    descriptor_map: [AtomicSpaceDescriptor; MAX_SPACES],
    space_state: [Mutex<SpaceState>; MAX_SPACES],
}

assert_impl_all!(Map64: Send, Sync);

impl Map64 {
    pub fn new() -> Self {
        let layout = vm_layout();
        let heap_end = layout.heap_end;
        let log_space_extent = layout.log_space_extent;
        assert!(
            log_space_extent <= usize::BITS as usize - LOG_MAX_SPACES,
            "log_space_extent ({log_space_extent}) exceeds address bits for MAX_SPACES"
        );
        let space_state = std::array::from_fn(|i| {
            // SAFETY: `i < MAX_SPACES` (`1 << LOG_MAX_SPACES`) and the `assert!` above verified
            // `log_space_extent <= usize::BITS as usize - LOG_MAX_SPACES`, so
            // `i << log_space_extent` cannot overflow `usize`. `Address::from_usize` only wraps the
            // raw integer address without dereferencing memory.
            let base = unsafe { Address::from_usize(i << log_space_extent) };
            Mutex::new(SpaceState {
                base_address: base,
                high_water: base,
                freelist_created: false,
            })
        });
        let descriptor_map =
            std::array::from_fn(|_| AtomicSpaceDescriptor::new(SpaceDescriptor::UNINITIALIZED));

        Self {
            heap_end,
            log_space_extent,
            finalized: AtomicBool::new(false),
            descriptor_map,
            space_state,
        }
    }
}

impl VMMap for Map64 {
    fn insert(&self, start: Address, extent: usize, descriptor: SpaceDescriptor) {
        assert!(
            self.is_space_start(start),
            "Space start {start} is not aligned to space boundary"
        );
        assert!(extent <= self.space_size());
        assert!(extent >= self.min_contiguous_extent());
        let index = self
            .space_index(start)
            .expect("Space start exceeds heap_end");
        self.descriptor_map[index]
            .compare_exchange(
                SpaceDescriptor::UNINITIALIZED,
                descriptor,
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .unwrap_or_else(|existing| {
                panic!(
                    "Conflicting virtual address request at space {start} (index {index}, existing {existing:?})"
                )
            });
    }

    fn create_freelist(&'static self, start: Address) -> Box<dyn FreeList> {
        let units = self.space_size() >> LOG_BYTES_IN_PAGE;
        self.create_parent_freelist(start, units, units as _)
    }

    fn create_parent_freelist(
        &self,
        start: Address,
        units: usize,
        grain: i32,
    ) -> Box<dyn FreeList> {
        assert!(
            self.is_space_start(start) && start.is_aligned_to(BYTES_IN_CHUNK),
            "Space start {start} must be space-aligned and chunk-aligned"
        );
        assert!(units >= conversions::bytes_to_pages_up(self.min_contiguous_extent()));

        let index = self
            .space_index(start)
            .expect("Space start exceeds heap_end");
        assert_ne!(
            self.descriptor_map[index].load(Ordering::Acquire),
            SpaceDescriptor::UNINITIALIZED,
            "create_parent_freelist called for space index {index} ({start}) before Map64::insert"
        );
        let mut state = self.space_state[index].lock().unwrap();
        assert!(
            !state.freelist_created,
            "create_parent_freelist called more than once for space index {index} ({start})"
        );
        state.freelist_created = true;

        let units = units.min(freelist::MAX_UNITS as usize) as i32;
        let grain = grain.min(units);
        let heads = 1;
        let pages_per_block = RawMemoryFreeList::default_block_size(units, heads);
        Box::new(RawMemoryFreeList::new(
            pages_per_block,
            units,
            grain,
            heads,
            MmapStrategy::RAW_MEMORY_FREELIST,
        ))
    }

    fn allocate_contiguous_chunks(
        &self,
        descriptor: SpaceDescriptor,
        chunks: usize,
        maybe_freelist: Option<&mut dyn FreeList>,
    ) -> Option<AllocatedChunkRegion> {
        if chunks == 0 {
            return None;
        }
        let index = descriptor.get_index();
        assert_eq!(self.space_index(descriptor.get_start()), Some(index));

        let mut state = self.space_state[index].lock().unwrap();
        let rtn = state.high_water;
        let extent = chunks << LOG_BYTES_IN_CHUNK;
        state.high_water = rtn + extent;
        let base_address = state.base_address;

        if let Some(freelist) = maybe_freelist {
            let Some(rmfl) = freelist.downcast_mut::<RawMemoryFreeList>() else {
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
        Some(AllocatedChunkRegion::new(rtn, chunks))
    }

    fn free_contiguous_chunks(&self, _region: AllocatedChunkRegion) -> usize {
        unreachable!("Map64 spaces are contiguous and do not free chunks")
    }

    fn get_available_discontiguous_chunks(&self) -> usize {
        panic!("We don't use discontiguous chunks for 64-bit!");
    }

    fn get_chunk_consumer_count(&self) -> usize {
        panic!("We don't use discontiguous chunks for 64-bit!");
    }

    fn finalize_static_space_map(
        &self,
        _from: Address,
        _to: Address,
        _on_discontig_start_determined: &mut dyn FnMut(Address),
    ) {
        self.finalized.store(true, Ordering::Release);
    }

    fn is_finalized(&self) -> bool {
        self.finalized.load(Ordering::Acquire)
    }

    fn get_descriptor_for_address(&self, address: Address) -> SpaceDescriptor {
        if let Some(index) = self.space_index(address) {
            self.descriptor_map
                .get(index)
                .map(|slot| slot.load(Ordering::Relaxed))
                .unwrap_or(SpaceDescriptor::UNINITIALIZED)
        } else {
            SpaceDescriptor::UNINITIALIZED
        }
    }
}

impl Map64 {
    #[inline]
    fn space_size(&self) -> usize {
        1usize << self.log_space_extent
    }

    #[inline]
    fn space_mask(&self) -> usize {
        ((1usize << LOG_MAX_SPACES) - 1) << self.log_space_extent
    }

    #[inline]
    fn space_index(&self, addr: Address) -> Option<usize> {
        if addr > self.heap_end {
            return None;
        }
        Some(addr >> self.log_space_extent)
    }

    #[inline]
    fn is_space_start(&self, base: Address) -> bool {
        (base & !self.space_mask()) == 0
    }
}

impl Default for Map64 {
    fn default() -> Self {
        Self::new()
    }
}
