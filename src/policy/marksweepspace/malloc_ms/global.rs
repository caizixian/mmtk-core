use super::metadata::*;
use crate::global_state::GcStatus;
use crate::plan::tracing::{ObjectQueue, OptionObjectQueue};
use crate::policy::sft::GCWorkerMutRef;
use crate::policy::sft::SFT;
use crate::policy::space::{CommonSpace, Space};
use crate::scheduler::{GCWork, GCWorkScheduler, GCWorker};
use crate::util::constants::BYTES_IN_PAGE;
use crate::util::copy::CopySemantics;
use crate::util::heap::chunk_map::{Chunk, ChunkMap};
use crate::util::heap::gc_trigger::GCTrigger;
use crate::util::heap::layout::vm_layout::BYTES_IN_CHUNK;
use crate::util::heap::space_descriptor::SpaceDescriptor;
use crate::util::heap::PageResource;
use crate::util::linear_scan::Region;
use crate::util::malloc::library::{BYTES_IN_MALLOC_PAGE, LOG_BYTES_IN_MALLOC_PAGE};
use crate::util::malloc::malloc_ms_util::*;
use crate::util::metadata::side_metadata::{
    self, SideMetadataContext, SideMetadataSanity, SideMetadataSpec,
};
use crate::util::metadata::vo_bit::VO_BIT_SIDE_METADATA_SPEC;
use crate::util::metadata::MetadataSpec;
use crate::util::object_enum::ObjectEnumerator;
use crate::util::opaque_pointer::*;
use crate::util::{conversions, metadata, Address, ObjectReference};
use crate::vm::{ActivePlan, Collection, ObjectModel, VMBinding};
use crate::MMTK;
#[cfg(debug_assertions)]
use std::collections::HashMap;
use std::collections::HashSet;
use std::marker::PhantomData;
#[cfg(debug_assertions)]
use std::sync::atomic::AtomicU32;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

// If true, we will use a hashmap to store all the allocated memory from malloc, and use it
// to make sure our allocation is correct.
#[cfg(debug_assertions)]
const ASSERT_ALLOCATION: bool = false;

const VO_REGION_SHIFT: usize = VO_BIT_SIDE_METADATA_SPEC.log_bytes_in_region;
const VO_META_BYTES_PER_CHUNK: usize = BYTES_IN_CHUNK >> (VO_REGION_SHIFT + 3);

const _: () = assert!((1usize << VO_REGION_SHIFT) == ObjectReference::ALIGNMENT);
const _: () = assert!(VO_META_BYTES_PER_CHUNK % 16 == 0);

use super::super::ChunkSweepLease;

/// This space uses malloc to get new memory, and performs mark-sweep for the memory.
pub struct MallocSpace<VM: VMBinding> {
    phantom: PhantomData<VM>,
    active_bytes: AtomicUsize,
    active_pages: AtomicUsize,
    metadata: SideMetadataContext,
    /// Work packet scheduler
    scheduler: Arc<GCWorkScheduler<VM>>,
    gc_trigger: Arc<GCTrigger<VM>>,
    descriptor: SpaceDescriptor,
    chunk_map: ChunkMap,
    mmap_metadata_lock: Mutex<()>,
    sweeping_chunks: Mutex<HashSet<Chunk>>,
    sft_ref: OnceLock<&'static (dyn SFT + Sync + 'static)>,
    // Mapping between allocated address and its size - this is used to check correctness.
    // Size will be set to zero when the memory is freed.
    #[cfg(debug_assertions)]
    active_mem: Mutex<HashMap<Address, usize>>,
    // The following fields are used for checking correctness of the parallel sweep implementation
    // as we need to check how many live bytes exist against `active_bytes` when the last sweep
    // work packet is executed
    #[cfg(debug_assertions)]
    pub total_work_packets: AtomicU32,
    #[cfg(debug_assertions)]
    pub completed_work_packets: AtomicU32,
    #[cfg(debug_assertions)]
    pub work_live_bytes: AtomicUsize,
}

impl<VM: VMBinding> SFT for MallocSpace<VM> {
    fn name(&self) -> &'static str {
        self.get_name()
    }

    fn is_live(&self, object: ObjectReference) -> bool {
        is_marked::<VM>(object, Ordering::SeqCst)
    }

    #[cfg(feature = "object_pinning")]
    fn pin_object(&self, _object: ObjectReference) -> bool {
        false
    }

    #[cfg(feature = "object_pinning")]
    fn unpin_object(&self, _object: ObjectReference) -> bool {
        false
    }

    #[cfg(feature = "object_pinning")]
    fn is_object_pinned(&self, _object: ObjectReference) -> bool {
        true
    }

    fn is_movable(&self) -> bool {
        false
    }

    #[cfg(feature = "sanity")]
    fn is_sane(&self) -> bool {
        true
    }

    // For malloc space, we need to further check the VO bit.
    fn is_in_space(&self, object: ObjectReference) -> bool {
        self.is_alloced_by_malloc(object)
    }

    /// For malloc space, we just use the side metadata.
    #[cfg(feature = "vo_bit")]
    fn is_mmtk_object(&self, addr: Address) -> Option<ObjectReference> {
        debug_assert!(!addr.is_zero());
        // `addr` cannot be mapped by us. It should be mapped by the malloc library.
        debug_assert!(!addr.is_mapped());
        self.has_object_alloced_by_malloc(addr)
    }

    #[cfg(feature = "vo_bit")]
    fn find_object_from_internal_pointer(
        &self,
        ptr: Address,
        max_search_bytes: usize,
    ) -> Option<ObjectReference> {
        crate::util::metadata::vo_bit::find_object_from_internal_pointer::<VM>(
            ptr,
            max_search_bytes,
        )
    }

    fn initialize_object_metadata(&self, object: ObjectReference, _bytes: usize) {
        trace!("initialize_object_metadata for object {}", object);
        set_vo_bit(object);
    }

    fn sft_trace_object(
        &self,
        queue: &mut OptionObjectQueue,
        object: ObjectReference,
        _worker: GCWorkerMutRef,
    ) -> ObjectReference {
        self.trace_object(queue, object)
    }
}

impl<VM: VMBinding> Space<VM> for MallocSpace<VM> {
    fn as_space(&self) -> &dyn Space<VM> {
        self
    }

    fn as_sft(&self) -> &(dyn SFT + Sync + 'static) {
        self
    }

    fn get_page_resource(&self) -> &dyn PageResource<VM> {
        unreachable!()
    }

    fn maybe_get_page_resource_mut(&mut self) -> Option<&mut dyn PageResource<VM>> {
        None
    }

    fn common(&self) -> &CommonSpace<VM> {
        unreachable!()
    }

    fn get_gc_trigger(&self) -> &GCTrigger<VM> {
        self.gc_trigger.as_ref()
    }

    fn initialize_sft(&'static self) {
        let _ = self.sft_ref.set(self);
        crate::mmtk::SFT_MAP.notify_space_creation_static(self);
    }

    fn release_multiple_pages(&mut self, _start: Address) {
        unreachable!()
    }

    // We have assertions in a debug build. We allow this pattern for the release build.
    #[allow(clippy::let_and_return)]
    fn in_space(&self, object: ObjectReference) -> bool {
        let ret = self.is_alloced_by_malloc(object);

        #[cfg(debug_assertions)]
        if ASSERT_ALLOCATION {
            let addr = object.to_object_start::<VM>();
            let active_mem = self.active_mem.lock().unwrap();
            if ret {
                // The VO bit tells that the object is in space.
                debug_assert!(
                    *active_mem.get(&addr).unwrap() != 0,
                    "active mem check failed for {} (object {}) - was freed",
                    addr,
                    object
                );
            } else {
                // The VO bit tells that the object is not in space. It could never be allocated, or have been freed.
                debug_assert!(
                    (!active_mem.contains_key(&addr))
                        || (active_mem.contains_key(&addr) && *active_mem.get(&addr).unwrap() == 0),
                    "mem check failed for {} (object {}): allocated = {}, size = {:?}",
                    addr,
                    object,
                    active_mem.contains_key(&addr),
                    if active_mem.contains_key(&addr) {
                        active_mem.get(&addr)
                    } else {
                        None
                    }
                );
            }
        }
        ret
    }

    fn address_in_space(&self, _start: Address) -> bool {
        unreachable!("We do not know if an address is in malloc space. Use in_space() to check if an object is in malloc space.")
    }

    fn get_name(&self) -> &'static str {
        "MallocSpace"
    }

    fn estimate_side_meta_pages(&self, data_pages: usize) -> usize {
        self.metadata.calculate_reserved_pages(data_pages)
    }

    #[allow(clippy::assertions_on_constants)]
    fn reserved_pages(&self) -> usize {
        use crate::util::constants::LOG_BYTES_IN_PAGE;
        // Assume malloc pages are no smaller than 4K pages. Otherwise the substraction below will fail.
        debug_assert!(LOG_BYTES_IN_MALLOC_PAGE >= LOG_BYTES_IN_PAGE);
        let data_pages = self.active_pages.load(Ordering::SeqCst)
            << (LOG_BYTES_IN_MALLOC_PAGE - LOG_BYTES_IN_PAGE);
        let meta_pages = self.estimate_side_meta_pages(data_pages);
        data_pages + meta_pages
    }

    fn verify_side_metadata_sanity(&self, side_metadata_sanity_checker: &mut SideMetadataSanity) {
        side_metadata_sanity_checker
            .verify_metadata_context(std::any::type_name::<Self>(), &self.metadata)
    }

    fn enumerate_objects(&self, _enumerator: &mut dyn ObjectEnumerator) {
        unimplemented!()
    }

    fn clear_side_log_bits(&self) {
        unimplemented!()
    }

    fn set_side_log_bits(&self) {
        unimplemented!()
    }
}

impl<VM: VMBinding> crate::policy::gc_work::PolicyTraceObject<VM> for MallocSpace<VM> {
    fn trace_object<Q: ObjectQueue, const KIND: crate::policy::gc_work::TraceKind>(
        &self,
        queue: &mut Q,
        object: ObjectReference,
        _copy: Option<CopySemantics>,
        _worker: &mut GCWorker<VM>,
    ) -> ObjectReference {
        self.trace_object(queue, object)
    }

    fn may_move_objects<const KIND: crate::policy::gc_work::TraceKind>() -> bool {
        false
    }
}

// Actually no max object size.
#[allow(dead_code)]
pub const MAX_OBJECT_SIZE: usize = usize::MAX;

impl<VM: VMBinding> MallocSpace<VM> {
    pub fn extend_global_side_metadata_specs(specs: &mut Vec<SideMetadataSpec>) {
        // MallocSpace needs to use VO bit. If the feature is turned on, the VO bit spec is in the global specs.
        // Otherwise, we manually add it.
        if !cfg!(feature = "vo_bit") {
            specs.push(VO_BIT_SIDE_METADATA_SPEC);
        }
    }

    pub fn new(args: crate::policy::space::PlanCreateSpaceArgs<VM>) -> Self {
        if *args.options.count_live_bytes_in_gc {
            // The implementation of counting live bytes needs a SpaceDescriptor which we do not have for MallocSpace.
            // Besides we cannot meaningfully measure the live bytes vs total pages for MallocSpace.
            panic!("count_live_bytes_in_gc is not supported by MallocSpace");
        }
        assert!(
            args.global_side_metadata_specs
                .contains(&VO_BIT_SIDE_METADATA_SPEC),
            "MallocSpace requires VO_BIT_SIDE_METADATA_SPEC in global_side_metadata_specs"
        );
        let descriptor = SpaceDescriptor::create_descriptor();
        let chunk_map = ChunkMap::new(descriptor.get_index());
        MallocSpace {
            phantom: PhantomData,
            active_bytes: AtomicUsize::new(0),
            active_pages: AtomicUsize::new(0),
            metadata: SideMetadataContext {
                global: args.global_side_metadata_specs.clone(),
                local: metadata::extract_side_metadata(&[
                    MetadataSpec::OnSide(ACTIVE_PAGE_METADATA_SPEC),
                    MetadataSpec::OnSide(OFFSET_MALLOC_METADATA_SPEC),
                    *VM::VMObjectModel::LOCAL_MARK_BIT_SPEC,
                ]),
            },
            scheduler: args.scheduler.clone(),
            gc_trigger: args.gc_trigger,
            descriptor,
            chunk_map,
            mmap_metadata_lock: Mutex::new(()),
            sweeping_chunks: Mutex::new(HashSet::new()),
            sft_ref: OnceLock::new(),
            #[cfg(debug_assertions)]
            active_mem: Mutex::new(HashMap::new()),
            #[cfg(debug_assertions)]
            total_work_packets: AtomicU32::new(0),
            #[cfg(debug_assertions)]
            completed_work_packets: AtomicU32::new(0),
            #[cfg(debug_assertions)]
            work_live_bytes: AtomicUsize::new(0),
        }
    }

    /// Set multiple pages, starting from the given address, for the given size, and increase the active page count if we set any page mark in the region.
    /// This is a thread-safe method, and can be used during mutator phase when mutators may access the same page.
    /// Performance-wise, this method may impose overhead, as we are doing a compare-exchange for every page in the range.
    fn set_page_mark(&self, start: Address, size: usize) {
        // Set first page
        let mut page = start.align_down(BYTES_IN_MALLOC_PAGE);
        let mut used_pages = 0;

        // It is important to go to the end of the object, which may span a page boundary
        while page < start + size {
            if compare_exchange_set_page_mark(page) {
                used_pages += 1;
            }

            page += BYTES_IN_MALLOC_PAGE;
        }

        if used_pages != 0 {
            self.active_pages.fetch_add(used_pages, Ordering::SeqCst);
        }
    }

    pub fn alloc(&self, tls: VMThread, size: usize, align: usize, offset: usize) -> Address {
        // TODO: Should refactor this and Space.acquire()
        if self.get_gc_trigger().poll(false, Some(self)) {
            assert!(VM::VMActivePlan::is_mutator(tls), "Polling in GC worker");
            VM::VMCollection::block_for_gc(VMMutatorThread(tls));
            return Address::ZERO;
        }

        let (address, actual_size, is_offset_malloc) = alloc_with_usable_size(size, align, offset);
        if !address.is_zero() {
            assert!(
                actual_size > 0,
                "malloc_usable_size returned 0 for non-null allocation {}",
                address
            );
            assert!(
                address.as_usize().checked_add(actual_size).is_some(),
                "MallocSpace allocation range overflow: {} + {}",
                address,
                actual_size
            );

            if !self.is_meta_space_mapped(address, actual_size) {
                // Map side metadata, register SFT entries, and publish `ChunkMap` allocation under
                // `mmap_metadata_lock` before any lock-free reader can observe the chunk as allocated.
                self.map_metadata_and_update_bound(address, actual_size);
            }

            // Set page marks for current object
            self.set_page_mark(address, actual_size);
            self.active_bytes.fetch_add(actual_size, Ordering::SeqCst);

            if is_offset_malloc {
                set_offset_malloc_bit(address);
            }

            #[cfg(debug_assertions)]
            if ASSERT_ALLOCATION {
                debug_assert!(actual_size != 0);
                self.active_mem.lock().unwrap().insert(address, actual_size);
            }
        }

        address
    }

    /// Check if metadata is mapped for a range [addr, addr + size). Metadata is mapped per chunk,
    /// we will go through all the chunks for [address, address + size), and check if they are mapped.
    /// If any of the chunks is not mapped, return false. Otherwise return true.
    fn is_meta_space_mapped(&self, address: Address, size: usize) -> bool {
        if size == 0 {
            return self.is_meta_space_mapped_for_address(address);
        }
        let first_chunk = address.chunk_index();
        let last_chunk = (address + (size - 1)).chunk_index();
        for chunk_idx in first_chunk..=last_chunk {
            let chunk = conversions::chunk_index_to_address(chunk_idx);
            if !self.is_meta_space_mapped_for_address(chunk) {
                return false;
            }
        }
        true
    }

    /// Safely verify that `chunk` is non-zero, allocated in `self.chunk_map` (`Acquire` load
    /// synchronizing-with `map_metadata_and_update_bound`), and registered to `self` in `SFT_MAP`
    /// (disambiguating any 4-bit `ChunkState` `space_index` collision with contiguous or other
    /// discontiguous spaces).
    #[inline]
    fn owns_chunk(&self, chunk: Chunk) -> bool {
        self.chunk_map.is_allocated_checked(chunk)
            && crate::mmtk::SFT_MAP.has_sft_entry(chunk.start())
            && std::ptr::eq(
                (crate::mmtk::SFT_MAP.get_checked(chunk.start()) as *const dyn SFT).cast::<()>(),
                (self as *const Self).cast::<()>(),
            )
    }

    /// Check if metadata is mapped and owned by `self` for `address` via `self.chunk_map` and `SFT_MAP`.
    fn is_meta_space_mapped_for_address(&self, address: Address) -> bool {
        let chunk_start = address.align_down(BYTES_IN_CHUNK);
        self.owns_chunk(Chunk::from_aligned_address(chunk_start))
    }

    pub fn trace_object<Q: ObjectQueue>(
        &self,
        queue: &mut Q,
        object: ObjectReference,
    ) -> ObjectReference {
        assert!(
            self.in_space(object),
            "Cannot mark an object {} that was not alloced by malloc.",
            object,
        );

        if !is_marked::<VM>(object, Ordering::Relaxed) {
            set_mark_bit::<VM>(object, Ordering::SeqCst);
            queue.enqueue(object);
        }

        object
    }

    fn map_metadata_and_update_bound(&self, addr: Address, size: usize) {
        assert!(size > 0, "map_metadata_and_update_bound requires size > 0");
        // Acquire the lock before mapping metadata, updating SFT_MAP, and publishing `chunk_map`.
        let _lock = self.mmap_metadata_lock.lock().unwrap();

        let map_metadata_space_for_chunk = |start: Address| {
            debug_assert!(start.is_aligned_to(BYTES_IN_CHUNK));
            assert!(!start.is_zero(), "MallocSpace chunk start cannot be zero");
            if self.is_meta_space_mapped_for_address(start) {
                return;
            }
            // 1. Ensure global and local side metadata are mapped for the chunk.
            let mmap_metadata_result =
                self.metadata
                    .try_map_metadata_space(start, BYTES_IN_CHUNK, self.get_name());
            assert!(
                mmap_metadata_result.is_ok(),
                "mmap sidemetadata failed for chunk_start ({}): {:?}",
                start,
                mmap_metadata_result
            );
            // 2. Register `self` in `SFT_MAP` *before* setting the `ChunkMap` allocation entry, so
            //    any concurrent mutator observing `is_meta_space_mapped == true` (via `Acquire`
            //    load on `chunk_map`) is guaranteed to see both mapped metadata and a live SFT entry.
            assert!(
                crate::mmtk::SFT_MAP.has_sft_entry(start),
                "MallocSpace chunk {} is outside SFT_MAP bounds",
                start
            );
            let sft = *self.sft_ref.get().expect(
                "Space::initialize_sft must be called before MallocSpace chunk registration",
            );
            crate::mmtk::SFT_MAP.update_static(sft, start, BYTES_IN_CHUNK);

            // 3. Publish the chunk mark last (`Ordering::Release` in `ChunkMap::set_allocated`).
            trace!("set chunk mark bit for {}", start);
            self.chunk_map
                .set_allocated(Chunk::from_aligned_address(start), true);
        };

        // Go through each chunk covered by `[addr, addr + size)` without pointer addition wraparound.
        let first_chunk = addr.chunk_index();
        let last_chunk = (addr + (size - 1)).chunk_index();
        for chunk_idx in first_chunk..=last_chunk {
            map_metadata_space_for_chunk(conversions::chunk_index_to_address(chunk_idx));
        }
    }

    /// Check if a given object was allocated by malloc
    pub fn is_alloced_by_malloc(&self, object: ObjectReference) -> bool {
        self.is_meta_space_mapped_for_address(object.to_raw_address())
            && crate::util::metadata::vo_bit::is_vo_bit_set(object)
    }

    /// Check if there is an object allocated by malloc at the address.
    ///
    /// This function doesn't check if `addr` is aligned.
    /// If not, it will try to load the VO bit for the address rounded down to the metadata's granularity.
    #[cfg(feature = "vo_bit")]
    pub fn has_object_alloced_by_malloc(&self, addr: Address) -> Option<ObjectReference> {
        if !self.is_meta_space_mapped_for_address(addr) {
            return None;
        }
        crate::util::metadata::vo_bit::is_vo_bit_set_for_addr(addr)
    }

    pub fn prepare(&self, _full_heap: bool) {}

    pub fn release(&self) {
        use crate::scheduler::WorkBucketStage;

        let space_descriptor = self.descriptor;
        let work_packets: Vec<Box<dyn GCWork<VM>>> = self
            .chunk_map
            .all_chunks()
            .filter(|&chunk| self.owns_chunk(chunk))
            .map(|chunk| -> Box<dyn GCWork<VM>> {
                Box::new(MSSweepChunk {
                    space_descriptor,
                    chunk,
                })
            })
            .collect();

        debug!("Generated {} sweep work packets", work_packets.len());
        #[cfg(debug_assertions)]
        {
            self.total_work_packets
                .store(work_packets.len() as u32, Ordering::SeqCst);
            self.completed_work_packets.store(0, Ordering::SeqCst);
            self.work_live_bytes.store(0, Ordering::SeqCst);
        }

        self.scheduler.work_buckets[WorkBucketStage::Release].bulk_add(work_packets);
    }

    pub fn end_of_gc(&self) {}

    /// Used when each chunk is done. Only called in debug build.
    #[cfg(debug_assertions)]
    fn debug_sweep_chunk_done(&self, live_bytes_in_the_chunk: usize) {
        debug!(
            "Used bytes after releasing: {}",
            self.active_bytes.load(Ordering::SeqCst)
        );

        let completed_packets = self.completed_work_packets.fetch_add(1, Ordering::SeqCst) + 1;
        self.work_live_bytes
            .fetch_add(live_bytes_in_the_chunk, Ordering::SeqCst);

        if completed_packets == self.total_work_packets.load(Ordering::Relaxed) {
            trace!(
                "work_live_bytes = {}, live_bytes = {}, active_bytes = {}",
                self.work_live_bytes.load(Ordering::Relaxed),
                live_bytes_in_the_chunk,
                self.active_bytes.load(Ordering::Relaxed)
            );
            debug_assert_eq!(
                self.work_live_bytes.load(Ordering::Relaxed),
                self.active_bytes.load(Ordering::Relaxed)
            );
        }
    }

    /// Project the mapped `VO_BIT` atomic bytes for an allocated `chunk` owned by `self`.
    fn chunk_vo_bytes(&self, chunk: Chunk) -> &[AtomicU8; VO_META_BYTES_PER_CHUNK] {
        assert!(
            !side_metadata::global_side_metadata_base_address().is_zero(),
            "MallocSpace::chunk_vo_bytes called before side metadata initialization"
        );
        let chunk_start = chunk.start();
        assert!(
            self.owns_chunk(chunk),
            "MallocSpace::chunk_vo_bytes requires an allocated chunk owned by MallocSpace: {:?}",
            chunk
        );
        let vo_meta_start =
            side_metadata::address_to_meta_address(&VO_BIT_SIDE_METADATA_SPEC, chunk_start);
        if VO_BIT_SIDE_METADATA_SPEC.uses_contiguous_side_metadata() {
            let spec_start = VO_BIT_SIDE_METADATA_SPEC.get_starting_address();
            let spec_end = VO_BIT_SIDE_METADATA_SPEC.upper_bound_address_for_contiguous();
            assert!(
                vo_meta_start >= spec_start
                    && vo_meta_start
                        .as_usize()
                        .checked_add(VO_META_BYTES_PER_CHUNK)
                        .is_some_and(|end| end <= spec_end.as_usize()),
                "VO_BIT metadata [{vo_meta_start}, + {VO_META_BYTES_PER_CHUNK}) for MallocSpace chunk {chunk:?} falls outside contiguous spec window [{spec_start}, {spec_end})"
            );
        }
        let first_page = vo_meta_start.align_down(BYTES_IN_PAGE);
        let last_page = (vo_meta_start + (VO_META_BYTES_PER_CHUNK - 1)).align_down(BYTES_IN_PAGE);
        let mut page = first_page;
        loop {
            assert!(
                crate::MMAPPER.is_mapped_address(page),
                "VO_BIT side-metadata page {} for MallocSpace chunk {:?} is not mapped",
                page,
                chunk
            );
            if page == last_page {
                break;
            }
            page += BYTES_IN_PAGE;
        }
        // SAFETY:
        // 1. `self.owns_chunk(chunk)` and the page-mapping loop above unconditionally verify that
        //    `chunk_start` is non-zero, chunk-aligned, owned by `self`, and that every OS page of
        //    `[vo_meta_start, vo_meta_start + VO_META_BYTES_PER_CHUNK)` is mapped (never unmapped
        //    for the process lifetime); `mapped_atomic_bytes_unchecked` additionally checks byte
        //    alignment, single-chunk containment, and bounds.
        // 2. All internal `VO_BIT` operations (`vo_bit::{set_vo_bit, unset_vo_bit*,
        //    is_vo_bit_set*, bzero_vo_bit, bcopy_vo_bit_from_mark_bit, scan_vo_bits_atomic,
        //    find_object_from_internal_pointer}`) access `VO_BIT_SIDE_METADATA_SPEC` exclusively
        //    via `AtomicU8`, and no `&[u8]` / `&mut [u8]` slice is created over `VO_BIT`.
        unsafe {
            VO_BIT_SIDE_METADATA_SPEC
                .mapped_atomic_bytes_unchecked::<VO_META_BYTES_PER_CHUNK>(chunk_start)
        }
    }

    /// Atomically sweep an allocated `chunk` owned by `self` during a STW `InPause` collection.
    fn sweep_allocated_chunk(&self, mmtk: &'static MMTK<VM>, chunk: Chunk) {
        assert_eq!(
            mmtk.get_gc_status(),
            GcStatus::InPause,
            "MallocSpace::sweep_allocated_chunk must execute during a STW GC pause"
        );
        let _lease = ChunkSweepLease::acquire(&self.sweeping_chunks, chunk, "MallocSpace");

        let chunk_start = chunk.start();
        let chunk_end = chunk_start + BYTES_IN_CHUNK;
        let vo_bytes = self.chunk_vo_bytes(chunk);

        #[cfg(debug_assertions)]
        let mut live_bytes = 0usize;

        debug!("Check active chunk {:?}", chunk_start);
        let mut empty_page_start = Address::ZERO;

        let mut byte_idx = 0usize;
        while byte_idx < VO_META_BYTES_PER_CHUNK {
            if (byte_idx & 15) == 0 && byte_idx + 16 <= VO_META_BYTES_PER_CHUNK {
                let any_nonzero = vo_bytes[byte_idx..byte_idx + 16]
                    .iter()
                    .fold(0u8, |acc, b| acc | b.load(Ordering::Relaxed));
                if any_nonzero == 0 {
                    byte_idx += 16;
                    continue;
                }
            }

            let mut bits = vo_bytes[byte_idx].load(Ordering::Relaxed);
            if bits == 0 {
                byte_idx += 1;
                continue;
            }

            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let addr = chunk_start + (((byte_idx << 3) + bit) << VO_REGION_SHIFT);
                let object = ObjectReference::from_raw_address(addr)
                    .expect("VO-marked address in non-zero MallocSpace chunk cannot be zero");

                let obj_start = object.to_object_start::<VM>();
                let is_offset = is_offset_malloc(obj_start);
                // SAFETY: `GcStatus::InPause` guarantees all mutators are stopped, `_lease`
                // guarantees exclusive sweep ownership of `chunk` (no concurrent sweeper on
                // `chunk`), and `self.owns_chunk(chunk)` guarantees `chunk` belongs to `self`.
                // Every set `VO_BIT` in `self` was set in `initialize_object_metadata` for a live
                // allocation from `alloc_with_usable_size` (with matching `is_offset`) and is
                // cleared via `unset_vo_bit` before freeing below.
                let bytes = unsafe { get_malloc_usable_size(obj_start, is_offset) };

                #[cfg(debug_assertions)]
                if ASSERT_ALLOCATION {
                    debug_assert!(
                        self.active_mem.lock().unwrap().contains_key(&obj_start),
                        "Address {} with VO bit is not in active_mem",
                        obj_start
                    );
                    debug_assert_eq!(
                        self.active_mem.lock().unwrap().get(&obj_start),
                        Some(&bytes),
                        "Address {} size in active_mem does not match malloc_usable_size",
                        obj_start
                    );
                }

                if !is_marked::<VM>(object, Ordering::Relaxed) {
                    trace!("Object {} has been allocated but not marked", object);
                    unset_vo_bit(object);
                    if is_offset {
                        unset_offset_malloc_bit(obj_start);
                    }
                    // SAFETY: Same live-allocation + `GcStatus::InPause` + `_lease` proof as above;
                    // `unset_vo_bit(object)` cleared the VO bit first so `obj_start` can never be
                    // visited or freed again.
                    unsafe {
                        free_allocation(obj_start, is_offset);
                    }
                    self.active_bytes.fetch_sub(bytes, Ordering::SeqCst);
                    #[cfg(debug_assertions)]
                    if ASSERT_ALLOCATION {
                        self.active_mem
                            .lock()
                            .unwrap()
                            .insert(obj_start, 0)
                            .unwrap();
                    }
                } else {
                    unset_mark_bit::<VM>(object);
                    #[cfg(debug_assertions)]
                    {
                        live_bytes += bytes;
                    }
                    if !empty_page_start.is_zero() {
                        let current_page =
                            obj_start.align_down(BYTES_IN_MALLOC_PAGE).min(chunk_end);
                        if current_page > empty_page_start {
                            let cleared =
                                unset_page_marks(empty_page_start, current_page - empty_page_start);
                            if cleared != 0 {
                                self.active_pages.fetch_sub(cleared, Ordering::SeqCst);
                            }
                        }
                    }
                    empty_page_start = (obj_start + bytes).align_up(BYTES_IN_MALLOC_PAGE);
                }
            }

            byte_idx += 1;
        }

        if !empty_page_start.is_zero() && empty_page_start < chunk_end {
            let cleared = unset_page_marks(empty_page_start, chunk_end - empty_page_start);
            if cleared != 0 {
                self.active_pages.fetch_sub(cleared, Ordering::SeqCst);
            }
        }

        if empty_page_start.is_zero() {
            let cleared = unset_page_marks(chunk_start, BYTES_IN_CHUNK);
            if cleared != 0 {
                self.active_pages.fetch_sub(cleared, Ordering::SeqCst);
            }
            self.chunk_map.set_allocated(chunk, false);
            crate::mmtk::SFT_MAP.clear_checked(chunk_start);
        }

        #[cfg(debug_assertions)]
        self.debug_sweep_chunk_done(live_bytes);
    }
}

fn with_malloc_space<VM: VMBinding, R>(
    mmtk: &'static MMTK<VM>,
    descriptor: SpaceDescriptor,
    f: impl FnOnce(&MallocSpace<VM>) -> R,
) -> R {
    let mut f = Some(f);
    let mut out = None;
    mmtk.get_plan().for_each_space(&mut |s| {
        if let Some(ms) = s.downcast_ref::<MallocSpace<VM>>() {
            if ms.descriptor == descriptor {
                out = Some((f.take().unwrap())(ms));
            }
        }
    });
    out.expect("MallocSpace must be registered in Plan::for_each_space")
}

/// Work packet that sweeps a single allocated 4 MiB chunk in `MallocSpace`.
pub struct MSSweepChunk {
    space_descriptor: SpaceDescriptor,
    chunk: Chunk,
}

impl<VM: VMBinding> GCWork<VM> for MSSweepChunk {
    fn do_work(&mut self, _worker: &mut GCWorker<VM>, mmtk: &'static MMTK<VM>) {
        with_malloc_space(mmtk, self.space_descriptor, |ms| {
            ms.sweep_allocated_chunk(mmtk, self.chunk);
        });
    }
}
