#![deny(unsafe_op_in_unsafe_fn)]

use super::metadata::*;
use crate::plan::tracing::{ObjectQueue, OptionObjectQueue};
use crate::policy::sft::GCWorkerMutRef;
use crate::policy::sft::SFT;
use crate::policy::space::CommonSpace;
use crate::scheduler::GCWorkScheduler;
use crate::util::heap::chunk_map::Chunk;
use crate::util::heap::chunk_map::ChunkMap;
use crate::util::heap::gc_trigger::GCTrigger;
use crate::util::heap::space_descriptor::SpaceDescriptor;
use crate::util::heap::PageResource;
use crate::util::linear_scan::Region;
use crate::util::malloc::library::{BYTES_IN_MALLOC_PAGE, LOG_BYTES_IN_MALLOC_PAGE};
use crate::util::malloc::malloc_ms_util::*;
use crate::util::metadata::side_metadata;
use crate::util::metadata::side_metadata::{
    SideMetadataContext, SideMetadataSanity, SideMetadataSpec,
};
use crate::util::metadata::vo_bit::VO_BIT_SIDE_METADATA_SPEC;
use crate::util::metadata::MetadataSpec;
use crate::util::object_enum::ObjectEnumerator;
use crate::util::opaque_pointer::*;
use crate::util::Address;
use crate::util::ObjectReference;
use crate::util::{conversions, metadata};
use crate::vm::VMBinding;
use crate::vm::{ActivePlan, Collection, ObjectModel};
use crate::{policy::space::Space, util::heap::layout::vm_layout::BYTES_IN_CHUNK};
#[cfg(debug_assertions)]
use std::collections::HashMap;
use std::marker::PhantomData;
#[cfg(debug_assertions)]
use std::sync::atomic::AtomicU32;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

// If true, we will use a hashmap to store all the allocated memory from malloc, and use it
// to make sure our allocation is correct.
#[cfg(debug_assertions)]
const ASSERT_ALLOCATION: bool = false;

const VO_REGION_SHIFT: usize = VO_BIT_SIDE_METADATA_SPEC.log_bytes_in_region;
const VO_META_BYTES_PER_CHUNK: usize = BYTES_IN_CHUNK >> (VO_REGION_SHIFT + 3);
const ACTIVE_PAGE_META_BYTES_PER_CHUNK: usize =
    BYTES_IN_CHUNK >> (LOG_BYTES_IN_MALLOC_PAGE as usize);

const _: () = assert!((1usize << VO_REGION_SHIFT) == ObjectReference::ALIGNMENT);
const _: () = assert!(VO_META_BYTES_PER_CHUNK % 16 == 0);

/// Linear `!Copy` capability witnessing exclusive STW sweep ownership of a single allocated
/// 4 MiB `Chunk` in `MallocSpace` together with mapped `MallocSpace` side metadata
/// (`VO_BIT_SIDE_METADATA_SPEC`, `ACTIVE_PAGE_METADATA_SPEC`, and optional `OnSide`
/// `LOCAL_MARK_BIT_SPEC`) across `[chunk.start(), chunk.end())`.
pub(super) struct MallocExclusiveChunk<'stw> {
    chunk: Chunk,
    on_side_mark_spec: Option<SideMetadataSpec>,
    _stw: PhantomData<&'stw mut ()>,
}

impl<'stw> MallocExclusiveChunk<'stw> {
    /// Mint an exclusive STW chunk token for `chunk`.
    ///
    /// # Safety
    /// * `chunk` must be a non-zero allocated chunk in `MallocSpace::chunk_map` with a valid
    ///   `SFT_MAP` entry, and all specs in `MallocSpace::metadata` (`VO_BIT_SIDE_METADATA_SPEC`,
    ///   `ACTIVE_PAGE_METADATA_SPEC`, `OFFSET_MALLOC_METADATA_SPEC`, and `on_side_mark_spec` if
    ///   `Some`) must be mapped and initialized across `[chunk.start(), chunk.end())` for `'stw`.
    /// * For the entire `'stw` lifetime, the holder must have unique ownership of `chunk`'s
    ///   chunk-local side metadata (`VO_BIT_SIDE_METADATA_SPEC`, `ACTIVE_PAGE_METADATA_SPEC`, and
    ///   `on_side_mark_spec`): mutators are stopped at STW `Release` and no other `MallocExclusiveChunk`
    ///   exists for `chunk`.
    /// * Every address in `[chunk.start(), chunk.end())` whose `VO_BIT` is `1` must be the
    ///   `ObjectReference` of a currently live (not yet `free`d) `MallocSpace` allocation.
    #[inline]
    unsafe fn new_unchecked(chunk: Chunk, on_side_mark_spec: Option<SideMetadataSpec>) -> Self {
        debug_assert!(chunk != Chunk::ZERO);
        Self {
            chunk,
            on_side_mark_spec,
            _stw: PhantomData,
        }
    }

    #[inline(always)]
    fn chunk(&self) -> Chunk {
        self.chunk
    }

    #[inline(always)]
    fn on_side_mark_spec(&self) -> Option<SideMetadataSpec> {
        self.on_side_mark_spec
    }

    /// Borrow all chunk-local side-metadata tables as disjoint `&mut [u8; ...]` slices.
    ///
    /// Taking `&mut self` statically prevents calling `tables_mut()` twice to create aliasing
    /// `&mut` slices over the same chunk's side metadata.
    fn tables_mut(&mut self) -> MallocChunkTables<'_> {
        let chunk_start = self.chunk.start();
        let vo_meta_start =
            side_metadata::address_to_meta_address(&VO_BIT_SIDE_METADATA_SPEC, chunk_start);
        let vo_meta_end = vo_meta_start + VO_META_BYTES_PER_CHUNK;

        let page_meta_start =
            side_metadata::address_to_meta_address(&ACTIVE_PAGE_METADATA_SPEC, chunk_start);
        let page_meta_end = page_meta_start + ACTIVE_PAGE_META_BYTES_PER_CHUNK;

        assert!(
            (
                VO_BIT_SIDE_METADATA_SPEC.is_global,
                VO_BIT_SIDE_METADATA_SPEC.offset
            ) != (
                ACTIVE_PAGE_METADATA_SPEC.is_global,
                ACTIVE_PAGE_METADATA_SPEC.offset
            ) && (vo_meta_end <= page_meta_start || page_meta_end <= vo_meta_start),
            "MallocSpace VO_BIT and ACTIVE_PAGE_METADATA_SPEC metadata ranges overlap"
        );

        let local_mark_bits = if let Some(mark_spec) = self.on_side_mark_spec {
            assert!(
                mark_spec.log_num_of_bits == 0 && mark_spec.log_bytes_in_region == VO_REGION_SHIFT,
                "OnSide LOCAL_MARK_BIT_SPEC geometry ({:?}) must match VO_BIT_SIDE_METADATA_SPEC",
                mark_spec
            );
            let mark_meta_start = side_metadata::address_to_meta_address(&mark_spec, chunk_start);
            let mark_meta_end = mark_meta_start + VO_META_BYTES_PER_CHUNK;
            assert!(
                (mark_spec.is_global, mark_spec.offset)
                    != (VO_BIT_SIDE_METADATA_SPEC.is_global, VO_BIT_SIDE_METADATA_SPEC.offset)
                    && (mark_spec.is_global, mark_spec.offset)
                        != (
                            ACTIVE_PAGE_METADATA_SPEC.is_global,
                            ACTIVE_PAGE_METADATA_SPEC.offset
                        )
                    && (mark_meta_end <= vo_meta_start || vo_meta_end <= mark_meta_start)
                    && (mark_meta_end <= page_meta_start || page_meta_end <= mark_meta_start),
                "OnSide LOCAL_MARK_BIT_SPEC metadata range overlaps VO_BIT or ACTIVE_PAGE_METADATA_SPEC"
            );
            // SAFETY: `MallocExclusiveChunk` guarantees `mark_spec` is mapped and initialized for
            // `[chunk_start, chunk_start + BYTES_IN_CHUNK)` (a single chunk on exact byte
            // boundaries), `&mut self` together with the disjointness assertions above guarantees
            // unique `&mut` access to `[mark_meta_start, mark_meta_end)`, and no raw-pointer
            // side-metadata access touches `mark_spec` while `MallocChunkTables` is live.
            let slice =
                unsafe { mark_spec.mapped_bytes_mut_unchecked(chunk_start, BYTES_IN_CHUNK) };
            Some(
                slice
                    .try_into()
                    .expect("OnSide LOCAL_MARK_BIT_SPEC chunk byte length mismatch"),
            )
        } else {
            None
        };

        // SAFETY: `MallocExclusiveChunk` guarantees `ACTIVE_PAGE_METADATA_SPEC` is mapped and
        // initialized for `[chunk_start, chunk_start + BYTES_IN_CHUNK)`, `&mut self` + the
        // disjointness assertions above guarantee unique `&mut` access, and no raw-pointer
        // side-metadata access touches `ACTIVE_PAGE_METADATA_SPEC` while `MallocChunkTables` is live.
        let active_pages_slice = unsafe {
            ACTIVE_PAGE_METADATA_SPEC.mapped_bytes_mut_unchecked(chunk_start, BYTES_IN_CHUNK)
        };
        // SAFETY: `MallocExclusiveChunk` guarantees `VO_BIT_SIDE_METADATA_SPEC` is mapped and
        // initialized for `[chunk_start, chunk_start + BYTES_IN_CHUNK)`, `&mut self` + the
        // disjointness assertions above guarantee unique `&mut` access, and no raw-pointer
        // side-metadata access touches `VO_BIT` while `MallocChunkTables` is live.
        let vo_bits_slice = unsafe {
            VO_BIT_SIDE_METADATA_SPEC.mapped_bytes_mut_unchecked(chunk_start, BYTES_IN_CHUNK)
        };

        MallocChunkTables {
            chunk_start,
            active_pages: active_pages_slice
                .try_into()
                .expect("ACTIVE_PAGE_METADATA_SPEC chunk byte length mismatch"),
            vo_bits: vo_bits_slice
                .try_into()
                .expect("VO_BIT_SIDE_METADATA_SPEC chunk byte length mismatch"),
            local_mark_bits,
            #[cfg(feature = "extreme_assertions")]
            cleared_vo_addrs: Vec::new(),
            #[cfg(feature = "extreme_assertions")]
            cleared_page_ranges: Vec::new(),
        }
    }
}

/// Disjoint mutable borrows of a single 4 MiB chunk's chunk-local side-metadata tables during STW
/// `MallocSpace` sweeping (`BP 2.4`, `#58`).
pub(super) struct MallocChunkTables<'a> {
    chunk_start: Address,
    active_pages: &'a mut [u8; ACTIVE_PAGE_META_BYTES_PER_CHUNK],
    vo_bits: &'a mut [u8; VO_META_BYTES_PER_CHUNK],
    local_mark_bits: Option<&'a mut [u8; VO_META_BYTES_PER_CHUNK]>,
    #[cfg(feature = "extreme_assertions")]
    cleared_vo_addrs: Vec<Address>,
    #[cfg(feature = "extreme_assertions")]
    cleared_page_ranges: Vec<(Address, usize)>,
}

impl<'a> MallocChunkTables<'a> {
    #[inline(always)]
    fn vo_bits(&self) -> &[u8; VO_META_BYTES_PER_CHUNK] {
        self.vo_bits
    }

    #[inline(always)]
    fn side_mark_bits(&self) -> Option<&[u8; VO_META_BYTES_PER_CHUNK]> {
        self.local_mark_bits.as_deref()
    }

    #[inline(always)]
    fn vo_bit_pos(&self, addr: Address) -> (usize, u8) {
        debug_assert!(addr >= self.chunk_start && addr < self.chunk_start + BYTES_IN_CHUNK);
        debug_assert!(addr.is_aligned_to(ObjectReference::ALIGNMENT));
        let region_idx = (addr - self.chunk_start) >> VO_REGION_SHIFT;
        let byte_idx = region_idx >> 3;
        let mask = 1u8 << (region_idx & 7);
        (byte_idx, mask)
    }

    #[inline(always)]
    fn is_vo_bit_set(&self, addr: Address) -> Option<ObjectReference> {
        let (byte_idx, mask) = self.vo_bit_pos(addr);
        if (self.vo_bits[byte_idx] & mask) != 0 {
            ObjectReference::from_raw_address(addr)
        } else {
            None
        }
    }

    #[inline]
    fn unset_vo_bit(&mut self, object: ObjectReference) {
        let addr = object.to_raw_address();
        let (byte_idx, mask) = self.vo_bit_pos(addr);
        debug_assert_ne!(
            self.vo_bits[byte_idx] & mask,
            0,
            "{:x}: VO bit not set",
            object
        );
        self.vo_bits[byte_idx] &= !mask;
        #[cfg(feature = "extreme_assertions")]
        self.cleared_vo_addrs.push(addr);
    }

    #[inline]
    fn is_marked<VM: VMBinding>(&self, object: ObjectReference) -> bool {
        if let Some(mark_bits) = self.local_mark_bits.as_deref() {
            let (byte_idx, mask) = self.vo_bit_pos(object.to_raw_address());
            (mark_bits[byte_idx] & mask) != 0
        } else {
            is_marked::<VM>(object, Ordering::Relaxed)
        }
    }

    #[inline]
    fn unset_mark_bit<VM: VMBinding>(&mut self, object: ObjectReference) {
        let addr = object.to_raw_address();
        if self.local_mark_bits.is_some() {
            let (byte_idx, mask) = self.vo_bit_pos(addr);
            if let Some(mark_bits) = self.local_mark_bits.as_deref_mut() {
                mark_bits[byte_idx] &= !mask;
            }
        } else {
            VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.store_atomic::<VM, u8>(
                object,
                0,
                None,
                Ordering::Relaxed,
            );
        }
    }

    #[inline]
    fn clear_all_side_mark_bits(&mut self) {
        if let Some(mark_bits) = self.local_mark_bits.as_deref_mut() {
            mark_bits.fill(0);
        }
    }

    /// Clear active-page bytes in `[start, start + size)` within this chunk and return how many
    /// active pages transitioned from `1` to `0`.
    fn unset_page_marks(&mut self, start: Address, size: usize) -> usize {
        assert!(
            start >= self.chunk_start
                && size <= (self.chunk_start + BYTES_IN_CHUNK) - start
                && start.is_aligned_to(BYTES_IN_MALLOC_PAGE)
                && conversions::raw_is_aligned(size, BYTES_IN_MALLOC_PAGE),
            "unset_page_marks range [{}, +{}) out of chunk {} bounds or unaligned",
            start,
            size,
            self.chunk_start
        );
        if size == 0 {
            return 0;
        }
        let start_idx = (start - self.chunk_start) >> (LOG_BYTES_IN_MALLOC_PAGE as usize);
        let page_count = size >> (LOG_BYTES_IN_MALLOC_PAGE as usize);
        let mut cleared_pages = 0;
        for page_byte in &mut self.active_pages[start_idx..start_idx + page_count] {
            if *page_byte != 0 {
                cleared_pages += 1;
                *page_byte = 0;
            }
        }
        #[cfg(feature = "extreme_assertions")]
        self.cleared_page_ranges.push((start, size));
        cleared_pages
    }

    /// Consume `self` (ending all `&mut [u8]` side-metadata borrows) and return recorded updates
    /// so `extreme_assertions` sanity tables can be synchronized without aliasing `&mut [u8]`.
    #[cfg(feature = "extreme_assertions")]
    fn into_sanity_updates(self) -> (Vec<Address>, Vec<(Address, usize)>) {
        (self.cleared_vo_addrs, self.cleared_page_ranges)
    }
}

/// This space uses malloc to get new memory, and performs mark-sweep for the memory.
pub struct MallocSpace<VM: VMBinding> {
    phantom: PhantomData<VM>,
    active_bytes: AtomicUsize,
    active_pages: AtomicUsize,
    metadata: SideMetadataContext,
    /// Work packet scheduler
    scheduler: Arc<GCWorkScheduler<VM>>,
    gc_trigger: Arc<GCTrigger<VM>>,
    #[allow(dead_code)]
    descriptor: SpaceDescriptor,
    chunk_map: ChunkMap,
    mmap_metadata_lock: Mutex<()>,
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

    fn initialize_sft(&self, _sft_map: &mut dyn crate::policy::sft_map::SFTMap) {
        // Do nothing - we will set sft when we get new results from malloc
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

use crate::scheduler::GCWorker;
use crate::util::copy::CopySemantics;

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

    #[inline(always)]
    fn on_side_mark_spec(&self) -> Option<SideMetadataSpec> {
        match *VM::VMObjectModel::LOCAL_MARK_BIT_SPEC {
            MetadataSpec::OnSide(spec) => Some(spec),
            MetadataSpec::InHeader(_) => None,
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

    fn set_chunk_mark(&self, start: Address, size: usize) {
        let mut chunk = start.align_down(BYTES_IN_CHUNK);
        while chunk < start + size {
            self.chunk_map
                .set_allocated(Chunk::from_aligned_address(chunk), true);
            chunk += BYTES_IN_CHUNK;
        }
    }

    pub fn alloc(&self, tls: VMThread, size: usize, align: usize, offset: usize) -> Address {
        // TODO: Should refactor this and Space.acquire()
        if self.get_gc_trigger().poll(false, Some(self)) {
            assert!(VM::VMActivePlan::is_mutator(tls), "Polling in GC worker");
            VM::VMCollection::block_for_gc(VMMutatorThread(tls));
            return Address::ZERO;
        }

        let (address, is_offset_malloc) = alloc::<VM>(size, align, offset);
        if !address.is_zero() {
            // SAFETY: `!address.is_zero()` and `(address, is_offset_malloc)` was just returned by
            // `malloc_ms_util::alloc` for a live allocation from the active malloc backend.
            let actual_size = unsafe { get_malloc_usable_size(address, is_offset_malloc) };

            if !self.is_meta_space_mapped(address, actual_size) {
                assert!(address.as_usize().checked_add(actual_size).is_some());
                // Map the metadata space for the associated chunk
                self.map_metadata_and_update_bound(address, actual_size);
                // Verify every chunk covered by `[address, address + actual_size)` has a valid SFT entry.
                if actual_size > 0 {
                    let first_chunk = address.chunk_index();
                    let last_chunk = (address + (actual_size - 1)).chunk_index();
                    for chunk_idx in first_chunk..=last_chunk {
                        assert!(crate::mmtk::SFT_MAP
                            .has_sft_entry(conversions::chunk_index_to_address(chunk_idx)));
                    }
                }
                // SAFETY: `self` is a heap-pinned `MallocSpace` inside `'static MMTK<VM>` registered
                // via `notify_space_creation` during `Plan::initialize_sft`, `map_metadata_and_update_bound`
                // mapped global/local side metadata for `[address, address + actual_size)`, and every
                // chunk in that range was just asserted to satisfy `has_sft_entry`.
                unsafe { crate::mmtk::SFT_MAP.update(self, address, actual_size) };
            }

            // Set chunk marks for the current object
            self.set_chunk_mark(address, actual_size);

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
        let mut chunk = address.align_down(BYTES_IN_CHUNK);
        while chunk < address + size {
            // is the chunk already mapped?
            if !self.is_meta_space_mapped_for_address(chunk) {
                return false;
            }
            chunk += BYTES_IN_CHUNK;
        }
        true
    }

    /// Check if metadata is mapped for a given address. We check with the chunk map: if the side metadata
    /// for the chunk map is mapped, and if it is allocated in the chunk map.
    fn is_meta_space_mapped_for_address(&self, address: Address) -> bool {
        let chunk_start = address.align_down(BYTES_IN_CHUNK);
        self.chunk_map
            .is_allocated_checked(Chunk::from_aligned_address(chunk_start))
    }

    /// Free an allocation starting at `addr` that was previously returned by [`Self::alloc`].
    ///
    /// # Safety
    /// - `addr` must be the exact non-zero allocation start address returned by [`Self::alloc`] for
    ///   a live (not yet freed) allocation in `self`, with mapped side metadata.
    /// - The allocation must not be accessed or freed again after this call.
    pub unsafe fn free(&self, addr: Address) {
        let offset_malloc_bit = is_offset_malloc(addr);
        // SAFETY: Caller guarantees `addr` is a non-zero live `MallocSpace` allocation whose
        // `OFFSET_MALLOC_METADATA_SPEC` bit `offset_malloc_bit` matches how `addr` was allocated.
        let bytes = unsafe { get_malloc_usable_size(addr, offset_malloc_bit) };
        // SAFETY: `addr`, `bytes`, and `offset_malloc_bit` correspond to a live allocation in `self`.
        unsafe { self.free_internal(addr, bytes, offset_malloc_bit) };
    }

    // XXX optimize: We pass the bytes in to free as otherwise there were multiple
    // indirect call instructions in the generated assembly
    /// Free a live allocation in `self` given its start address, usable byte size, and offset-malloc flag.
    ///
    /// # Safety
    /// - `addr` must be a non-zero live allocation returned by `malloc_ms_util::alloc` in `self`,
    ///   matching `offset_malloc_bit` and `bytes`, with mapped `OFFSET_MALLOC_METADATA_SPEC` metadata.
    /// - `addr` must not be accessed or freed again after this call.
    unsafe fn free_internal(&self, addr: Address, bytes: usize, offset_malloc_bit: bool) {
        if offset_malloc_bit {
            trace!("Free memory {:x}", addr);
            // SAFETY: Caller guarantees `addr` is a live offset allocation (`offset_malloc_bit`)
            // returned by `align_offset_alloc` and not freed again.
            unsafe { offset_free(addr) };
            unset_offset_malloc_bit(addr);
        } else {
            let ptr = addr.to_mut_ptr();
            trace!("Free memory {:?}", ptr);
            // SAFETY: Caller guarantees `addr` (`ptr`) was returned directly by `calloc`/`posix_memalign`
            // (`!offset_malloc_bit`) and has not yet been freed.
            unsafe {
                free(ptr);
            }
        }

        self.active_bytes.fetch_sub(bytes, Ordering::SeqCst);

        #[cfg(debug_assertions)]
        if ASSERT_ALLOCATION {
            self.active_mem.lock().unwrap().insert(addr, 0).unwrap();
        }
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
        // Acquire the lock before
        let _lock = self.mmap_metadata_lock.lock().unwrap();

        // Mmap metadata for each chunk
        let map_metadata_space_for_chunk = |start: Address| {
            debug_assert!(start.is_aligned_to(BYTES_IN_CHUNK));
            assert!(!start.is_zero(), "MallocSpace chunk start cannot be zero");
            // Ensure global and local side metadata are mapped for the chunk before setting the
            // chunk-map allocation entry (promoted from `debug_assert!` to unconditional `assert!`
            // so `is_allocated_checked` witnesses mapped metadata in release builds as well).
            let mmap_metadata_result =
                self.metadata
                    .try_map_metadata_space(start, BYTES_IN_CHUNK, self.get_name());
            assert!(
                mmap_metadata_result.is_ok(),
                "mmap sidemetadata failed for chunk_start ({}): {:?}",
                start,
                mmap_metadata_result
            );
            // Set the chunk mark at the end. So if we have chunk mark set, we know we have mapped side metadata
            // for the chunk.
            trace!("set chunk mark bit for {}", start);
            self.chunk_map
                .set_allocated(Chunk::from_aligned_address(start), true);
        };

        // Go through each chunk, and map for them.
        let mut chunk = conversions::chunk_align_down(addr);
        while chunk < addr + size {
            map_metadata_space_for_chunk(chunk);
            chunk += BYTES_IN_CHUNK;
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

    pub fn prepare(&mut self, _full_heap: bool) {}

    pub fn release(&mut self) {
        use crate::scheduler::WorkBucketStage;
        // Reborrow `self: &mut Self` as `shared_self: &Self` BEFORE extending its lifetime for
        // worker packets so `self: &mut Self` is never touched again after `&'static Self` exists
        // (preserving Stacked Borrows / Tree Borrows aliasing invariants).
        let shared_self: &Self = self;
        // SAFETY: Every `MallocSpace` lives inside the heap-pinned `Plan` owned by `'static MMTK<VM>`,
        // and all `WorkBucketStage::Release` tasks complete before `MMTK` could ever be dropped.
        let space: &'static Self = unsafe { &*(shared_self as *const Self) };
        let on_side_mark_spec = shared_self.on_side_mark_spec();
        let mut prev_chunk: Option<Chunk> = None;
        let mut work_packets: Vec<Box<dyn GCWork<VM>>> = Vec::new();
        for chunk in shared_self.chunk_map.all_chunks() {
            assert!(
                prev_chunk.is_none_or(|prev| prev < chunk),
                "ChunkMap::all_chunks must yield strictly monotonic chunks for disjoint MallocExclusiveChunk ownership"
            );
            prev_chunk = Some(chunk);
            assert!(
                shared_self.chunk_map.is_allocated_checked(chunk)
                    && crate::mmtk::SFT_MAP.has_sft_entry(chunk.start()),
                "Allocated MallocSpace chunk {:?} must have mapped metadata and an SFT entry",
                chunk
            );
            // SAFETY: `MallocSpace::release` runs during STW `Release` while all mutators are
            // parked; strictly monotonic `prev_chunk < chunk` proves each allocated `chunk` is
            // minted into at most one linear `MSSweepChunk` packet, `is_allocated_checked` +
            // `map_metadata_and_update_bound` guarantee `VO_BIT`, `ACTIVE_PAGE_METADATA_SPEC`,
            // `OFFSET_MALLOC_METADATA_SPEC`, and `on_side_mark_spec` are mapped across `chunk` for
            // `'static`, and every set `VO_BIT` in `chunk` denotes a live `MallocSpace` object.
            let exclusive =
                unsafe { MallocExclusiveChunk::new_unchecked(chunk, on_side_mark_spec) };
            work_packets.push(Box::new(MSSweepChunk {
                ms: space,
                chunk: Some(exclusive),
            }));
        }

        debug!("Generated {} sweep work packets", work_packets.len());
        #[cfg(debug_assertions)]
        {
            shared_self
                .total_work_packets
                .store(work_packets.len() as u32, Ordering::SeqCst);
            shared_self
                .completed_work_packets
                .store(0, Ordering::SeqCst);
            shared_self.work_live_bytes.store(0, Ordering::SeqCst);
        }

        shared_self.scheduler.work_buckets[WorkBucketStage::Release].bulk_add(work_packets);
    }

    pub fn end_of_gc(&mut self) {}

    /// Sweep all dead objects in the chunk starting at `chunk_start`.
    ///
    /// # Safety
    /// - `chunk_start` must be non-zero, aligned to `BYTES_IN_CHUNK`, and correspond to an
    ///   allocated chunk in `self.chunk_map` with mapped side metadata and a valid `SFT_MAP` entry.
    /// - Called during the STW `Release` stage by the unique worker assigned to `chunk_start`
    ///   (no mutators are allocating and no other thread accesses `chunk_start`'s chunk-local side
    ///   metadata), and every set `VO_BIT` in `chunk_start` must denote a live `MallocSpace` object.
    pub unsafe fn sweep_chunk(&self, chunk_start: Address) {
        assert!(
            !chunk_start.is_zero() && chunk_start.is_aligned_to(BYTES_IN_CHUNK),
            "sweep_chunk requires a non-zero chunk-aligned address: {}",
            chunk_start
        );
        let chunk = Chunk::from_aligned_address(chunk_start);
        assert!(
            self.chunk_map.is_allocated_checked(chunk)
                && crate::mmtk::SFT_MAP.has_sft_entry(chunk_start),
            "sweep_chunk requires an allocated MallocSpace chunk with an SFT entry: {:?}",
            chunk
        );
        // SAFETY: Caller upholds exclusive STW sweep ownership, mapped side metadata, and live
        // VO-marked object invariants for `chunk`.
        let exclusive =
            unsafe { MallocExclusiveChunk::new_unchecked(chunk, self.on_side_mark_spec()) };
        self.sweep_exclusive_chunk(exclusive);
    }

    /// Given a live (not-yet-freed) object in `MallocSpace`, return its malloc start address,
    /// whether it is an offset malloc, and its malloc usable size.
    ///
    /// # Safety
    /// `object` must be a currently allocated (not yet `free`d) object in `MallocSpace` with
    /// mapped `OFFSET_MALLOC_METADATA_SPEC` metadata for `object.to_object_start::<VM>()`.
    unsafe fn get_malloc_addr_size(object: ObjectReference) -> (Address, bool, usize) {
        let obj_start = object.to_object_start::<VM>();
        let offset_malloc_bit = is_offset_malloc(obj_start);
        // SAFETY: Caller guarantees `object` (`obj_start`) is a live `MallocSpace` allocation whose
        // `OFFSET_MALLOC_METADATA_SPEC` bit `offset_malloc_bit` matches how `obj_start` was allocated.
        let bytes = unsafe { get_malloc_usable_size(obj_start, offset_malloc_bit) };
        (obj_start, offset_malloc_bit, bytes)
    }

    /// Scan word-aligned addresses in `[start, end)` within `tables`'s chunk using safe 128-bit
    /// `[u64; 2]` zero-skipping on `tables.vo_bits()`, invoking `visit` for each VO-marked object.
    ///
    /// Queries `(obj_start, offset_malloc_bit, bytes)` *before* invoking `visit` because `visit`
    /// may sweep and `libc::free` dead objects.
    ///
    /// # Safety
    /// Every address in `[start, end)` whose `VO_BIT` is `1` in `tables` must be a currently
    /// allocated (not-yet-freed) `MallocSpace` object with mapped `OFFSET_MALLOC_METADATA_SPEC`
    /// metadata.
    unsafe fn for_each_object_in_chunk_range(
        tables: &mut MallocChunkTables<'_>,
        start: Address,
        end: Address,
        mut visit: impl FnMut(&mut MallocChunkTables<'_>, ObjectReference, Address, bool, usize),
    ) {
        let chunk_start = tables.chunk_start;
        debug_assert!(start >= chunk_start && start < end && end <= chunk_start + BYTES_IN_CHUNK);
        debug_assert!(start.is_aligned_to(ObjectReference::ALIGNMENT));
        debug_assert!(end.is_aligned_to(ObjectReference::ALIGNMENT));

        let bulk_load_size: usize = 128usize << VO_REGION_SHIFT;
        let mut cursor = start;
        while cursor < end {
            if cursor.is_aligned_to(bulk_load_size) && end - cursor >= bulk_load_size {
                let pair_idx = (cursor - chunk_start) / bulk_load_size;
                let pair = as_u64_pairs(tables.vo_bits())[pair_idx];
                if pair == [0, 0] {
                    cursor += bulk_load_size;
                    continue;
                }
            }

            if let Some(object) = tables.is_vo_bit_set(cursor) {
                // SAFETY: `object` has its VO bit set in `[start, end)`, so by caller invariant it
                // is a live `MallocSpace` allocation with mapped `OFFSET_MALLOC_METADATA_SPEC`; we
                // query its start address and usable size once before `visit` potentially frees it.
                let (obj_start, offset_malloc_bit, bytes) =
                    unsafe { Self::get_malloc_addr_size(object) };
                let next_cursor = (obj_start + bytes)
                    .align_up(ObjectReference::ALIGNMENT)
                    .max(cursor + ObjectReference::ALIGNMENT);
                visit(tables, object, obj_start, offset_malloc_bit, bytes);
                cursor = next_cursor;
            } else {
                cursor += ObjectReference::ALIGNMENT;
            }
        }
    }

    /// Sweep `object` if unmarked (freeing its malloc backing and clearing its VO bit in `tables`),
    /// or clear empty active-page bytes preceding a live `object` in `tables`.
    /// Returns `true` if `object` was swept (freed).
    ///
    /// # Safety
    /// `(obj_start, offset_malloc_bit, bytes)` must describe the live (not-yet-freed) `MallocSpace`
    /// allocation `object` in `tables`'s chunk, with mapped `OFFSET_MALLOC_METADATA_SPEC` metadata.
    unsafe fn sweep_object_with_tables(
        &self,
        tables: &mut MallocChunkTables<'_>,
        object: ObjectReference,
        obj_start: Address,
        offset_malloc_bit: bool,
        bytes: usize,
        empty_page_start: &mut Address,
    ) -> bool {
        if !tables.is_marked::<VM>(object) {
            // Dead object: free malloc memory and clear VO bit through `tables` (no raw VO_BIT pointer!).
            trace!("Object {} has been allocated but not marked", object);
            // SAFETY: `(obj_start, bytes, offset_malloc_bit)` describes the dead live allocation
            // `object`, which has not yet been freed and whose VO bit is cleared immediately below
            // so it will never be visited or freed again.
            unsafe { self.free_internal(obj_start, bytes, offset_malloc_bit) };
            trace!("free object {}", object);
            tables.unset_vo_bit(object);
            true
        } else {
            // Live object that we have marked: clear empty pages between previous live object and this one.
            if !empty_page_start.is_zero() {
                let chunk_end = tables.chunk_start + BYTES_IN_CHUNK;
                let current_page = obj_start.align_down(BYTES_IN_MALLOC_PAGE).min(chunk_end);
                if current_page > *empty_page_start {
                    let cleared = tables
                        .unset_page_marks(*empty_page_start, current_page - *empty_page_start);
                    if cleared != 0 {
                        self.active_pages.fetch_sub(cleared, Ordering::SeqCst);
                    }
                }
            }

            *empty_page_start = (obj_start + bytes).align_up(BYTES_IN_MALLOC_PAGE);
            false
        }
    }

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

    /// Sweep a chunk exclusively owned via a linear [`MallocExclusiveChunk`].
    ///
    /// Stacked Borrows discipline (`#58`): all chunk-local reads and writes to `VO_BIT`,
    /// `ACTIVE_PAGE_METADATA_SPEC`, and `OnSide` `LOCAL_MARK_BIT_SPEC` go exclusively through
    /// `MallocChunkTables<'_>` (`&mut [u8]` slices), while `OFFSET_MALLOC_METADATA_SPEC` uses
    /// strictly atomic operations on its disjoint metadata table. `tables` is dropped before any
    /// `SFT_MAP.clear` or `extreme_assertions` sanity-table synchronization runs.
    fn sweep_exclusive_chunk(&self, mut exclusive: MallocExclusiveChunk<'_>) {
        // We can do xor on bulk for mark bits and valid object bits. If the result is zero, that means
        // the objects in it are all alive (both valid object bit and mark bit is set), and we do not
        // need to do anything for the region. Otherwise, we will sweep each single object in the region.
        // Note: Enabling this would result in inaccurate page accounting. We disable this by default, and
        // we will sweep object one by one.
        const BULK_XOR_ON_MARK_BITS: bool = false;

        let chunk = exclusive.chunk();
        let chunk_start = chunk.start();
        let chunk_end = chunk_start + BYTES_IN_CHUNK;
        #[cfg(feature = "extreme_assertions")]
        let on_side_mark_spec = exclusive.on_side_mark_spec();

        #[cfg(debug_assertions)]
        let mut live_bytes = 0usize;

        debug!("Check active chunk {:?}", chunk_start);
        let mut empty_page_start = Address::ZERO;

        // Scope `tables` (`&mut [u8]` borrows) strictly inside this block so all mutable
        // side-metadata slices are dropped before any `SideMetadataSpec` raw-pointer sync or
        // `SFT_MAP.clear` runs below.
        #[cfg(feature = "extreme_assertions")]
        let (cleared_vo_addrs, cleared_page_ranges) = {
            let mut tables = exclusive.tables_mut();
            self.sweep_tables(
                &mut tables,
                chunk_start,
                chunk_end,
                BULK_XOR_ON_MARK_BITS,
                &mut empty_page_start,
                #[cfg(debug_assertions)]
                &mut live_bytes,
            );
            tables.into_sanity_updates()
        };
        #[cfg(not(feature = "extreme_assertions"))]
        {
            let mut tables = exclusive.tables_mut();
            self.sweep_tables(
                &mut tables,
                chunk_start,
                chunk_end,
                BULK_XOR_ON_MARK_BITS,
                &mut empty_page_start,
                #[cfg(debug_assertions)]
                &mut live_bytes,
            );
        }

        #[cfg(feature = "extreme_assertions")]
        {
            for addr in cleared_vo_addrs {
                VO_BIT_SIDE_METADATA_SPEC.store_atomic::<u8>(addr, 0, Ordering::Relaxed);
            }
            for (start, size) in cleared_page_ranges {
                ACTIVE_PAGE_METADATA_SPEC.bzero_metadata(start, size);
            }
            if let Some(mark_spec) = on_side_mark_spec {
                mark_spec.bzero_metadata(chunk_start, BYTES_IN_CHUNK);
            }
        }

        if empty_page_start.is_zero() {
            self.chunk_map.set_allocated(chunk, false);
            // SAFETY: `chunk_start` is chunk-aligned and was verified to have an SFT entry when
            // `MallocExclusiveChunk` was minted; this worker has exclusive STW sweep ownership of
            // `chunk`.
            unsafe { crate::mmtk::SFT_MAP.clear(chunk_start) };
        }

        #[cfg(debug_assertions)]
        self.debug_sweep_chunk_done(live_bytes);
    }

    fn sweep_tables(
        &self,
        tables: &mut MallocChunkTables<'_>,
        chunk_start: Address,
        chunk_end: Address,
        bulk_xor_on_mark_bits: bool,
        empty_page_start: &mut Address,
        #[cfg(debug_assertions)] live_bytes: &mut usize,
    ) {
        if bulk_xor_on_mark_bits && tables.side_mark_bits().is_some() {
            let bulk_load_size: usize = 128usize << VO_REGION_SHIFT;
            let num_pairs = VO_META_BYTES_PER_CHUNK / 16;

            for pair_idx in 0..num_pairs {
                let address = chunk_start + pair_idx * bulk_load_size;
                let vo_pair = as_u64_pairs(tables.vo_bits())[pair_idx];
                let mark_pair = as_u64_pairs(tables.side_mark_bits().unwrap())[pair_idx];

                if (vo_pair[0] ^ mark_pair[0]) | (vo_pair[1] ^ mark_pair[1]) != 0 {
                    let end = address + bulk_load_size;
                    let sweep_in_bulk = |tables: &mut MallocChunkTables<'_>,
                                         object: ObjectReference,
                                         obj_start: Address,
                                         offset_malloc_bit: bool,
                                         bytes: usize| {
                        // SAFETY: `(obj_start, offset_malloc_bit, bytes)` was queried from the
                        // live `object` immediately prior to this closure call.
                        unsafe {
                            self.sweep_object_with_tables(
                                tables,
                                object,
                                obj_start,
                                offset_malloc_bit,
                                bytes,
                                empty_page_start,
                            );
                        }
                    };
                    // SAFETY: `MallocExclusiveChunk` guarantees every VO-marked address in
                    // `[address, end)` is a live `MallocSpace` allocation with mapped metadata, and
                    // each dead object's VO bit is cleared immediately when freed.
                    unsafe {
                        Self::for_each_object_in_chunk_range(tables, address, end, sweep_in_bulk);
                    }
                } else if vo_pair != [0, 0] {
                    // TODO we aren't actually accounting for the case where an object is alive and spans
                    // a page boundary as we don't know what the object sizes are/what is alive in the bulk region
                    *empty_page_start = (*empty_page_start)
                        .max((address + bulk_load_size).align_up(BYTES_IN_MALLOC_PAGE));
                }
            }

            #[cfg(debug_assertions)]
            {
                let count_live = |tables: &mut MallocChunkTables<'_>,
                                  object: ObjectReference,
                                  obj_start: Address,
                                  _offset_malloc_bit: bool,
                                  bytes: usize| {
                    if ASSERT_ALLOCATION {
                        debug_assert!(
                            self.active_mem.lock().unwrap().contains_key(&obj_start),
                            "Address {} with VO bit is not in active_mem",
                            obj_start
                        );
                        debug_assert_eq!(
                            self.active_mem.lock().unwrap().get(&obj_start),
                            Some(&bytes),
                            "Address {} size in active_mem does not match the size from malloc_usable_size",
                            obj_start
                        );
                    }
                    debug_assert!(
                        tables.is_marked::<VM>(object),
                        "Dead object = {} found after sweep",
                        object
                    );
                    *live_bytes += bytes;
                };
                // SAFETY: All dead objects were swept above and had their VO bits cleared, so every
                // remaining VO-marked address in `[chunk_start, chunk_end)` is a live allocation.
                unsafe {
                    Self::for_each_object_in_chunk_range(
                        tables,
                        chunk_start,
                        chunk_end,
                        count_live,
                    );
                }
            }

            tables.clear_all_side_mark_bits();
        } else {
            let sweep_one = |tables: &mut MallocChunkTables<'_>,
                             object: ObjectReference,
                             obj_start: Address,
                             offset_malloc_bit: bool,
                             bytes: usize| {
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
                        "Address {} size in active_mem does not match the size from malloc_usable_size",
                        obj_start
                    );
                }

                // SAFETY: `(obj_start, offset_malloc_bit, bytes)` was queried from the live
                // `object` immediately before invoking this closure.
                let live = !unsafe {
                    self.sweep_object_with_tables(
                        tables,
                        object,
                        obj_start,
                        offset_malloc_bit,
                        bytes,
                        empty_page_start,
                    )
                };
                if live {
                    tables.unset_mark_bit::<VM>(object);
                    #[cfg(debug_assertions)]
                    {
                        *live_bytes += bytes;
                    }
                }
            };
            // SAFETY: `MallocExclusiveChunk` guarantees every VO-marked address in
            // `[chunk_start, chunk_end)` is a live `MallocSpace` allocation with mapped metadata.
            unsafe {
                Self::for_each_object_in_chunk_range(tables, chunk_start, chunk_end, sweep_one);
            }

            if !empty_page_start.is_zero() && *empty_page_start < chunk_end {
                // Clear trailing empty pages at the end of a chunk after the last live object.
                let cleared =
                    tables.unset_page_marks(*empty_page_start, chunk_end - *empty_page_start);
                if cleared != 0 {
                    self.active_pages.fetch_sub(cleared, Ordering::SeqCst);
                }
            }
        }

        if empty_page_start.is_zero() {
            let cleared = tables.unset_page_marks(chunk_start, BYTES_IN_CHUNK);
            if cleared != 0 {
                self.active_pages.fetch_sub(cleared, Ordering::SeqCst);
            }
        }
    }
}

use crate::scheduler::GCWork;
use crate::MMTK;

/// Simple work packet that sweeps a single exclusively-owned 4 MiB chunk.
pub struct MSSweepChunk<VM: VMBinding> {
    ms: &'static MallocSpace<VM>,
    chunk: Option<MallocExclusiveChunk<'static>>,
}

impl<VM: VMBinding> GCWork<VM> for MSSweepChunk<VM> {
    fn do_work(&mut self, _worker: &mut GCWorker<VM>, _mmtk: &'static MMTK<VM>) {
        let exclusive = self.chunk.take().expect(
            "MSSweepChunk::do_work must execute at most once per linear MallocExclusiveChunk",
        );
        self.ms.sweep_exclusive_chunk(exclusive);
    }
}
