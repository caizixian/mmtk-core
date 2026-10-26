use std::marker::PhantomData;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use super::sft::SFT;
use super::space::{CommonSpace, Space};
use crate::global_state::GcStatus;
use crate::plan::tracing::{ObjectQueue, OptionObjectQueue};
use crate::policy::gc_work::{TraceKind, TRACE_KIND_TRANSITIVE_PIN};
use crate::policy::sft::GCWorkerMutRef;
use crate::scheduler::{GCWorkScheduler, GCWorker, WorkersParked};
use crate::util::alloc::allocator::align_allocation_no_fill;
use crate::util::constants::{BYTES_IN_PAGE, LOG_BYTES_IN_WORD};
use crate::util::copy::CopySemantics;
use crate::util::heap::layout::vm_layout::BYTES_IN_CHUNK;
use crate::util::heap::{MonotonePageResource, PageResource};
use crate::util::metadata::{extract_side_metadata, vo_bit};
use crate::util::object_enum::{self, ObjectEnumerator};
use crate::util::{Address, ObjectReference};
use crate::vm::*;
use crate::MMAPPER;
use atomic::Ordering;

pub(crate) const TRACE_KIND_MARK: TraceKind = 0;
pub(crate) const TRACE_KIND_FORWARD: TraceKind = 1;

pub(crate) type ValidatedSpans = Arc<[(Address, Address)]>;

static NEXT_LISP2_EPOCH: AtomicU64 = AtomicU64::new(1);

/// Witness token proving forwarding-pointer header accesses occur during an active STW GC pause
/// without racing against `Lisp2Space::compact`'s non-atomic `VMObjectModel::copy_to` slides:
/// - [`Self::active_worker`]: borrows `worker: &'a mut GCWorker<VM>` of `space.scheduler`. Because
///   `GCWorker` instances (`0..worker_count`) are created solely by `WorkerGroup::create_workers`
///   and `WorkersParked` is yielded solely inside `WorkerMonitor::park_and_wait` while every
///   worker holds `&mut GCWorker<VM>`, `worker: &'a mut GCWorker<VM>` statically excludes
///   `Lisp2Space::compact(&self, parked: &mut WorkersParked<'_>)` with zero locks or atomic RMWs.
/// - [`Self::shared_external`]: holds `RwLockReadGuard` on `space.compact_rwlock` for cold external
///   queries (`SFT::get_forwarded_object`, `debug_print_object_info`).
/// - [`Self::exclusive_parked`]: holds `&'a mut WorkersParked<'_>` and `&'a mut RwLockWriteGuard`
///   inside `compact`, statically ending `'a` before `VMObjectModel::copy_to` runs.
#[derive(Clone, Copy)]
struct ForwardingAccess<'a> {
    extra_spans: &'a [(Address, Address)],
}

impl<'a> ForwardingAccess<'a> {
    #[inline]
    fn shared_external<VM: VMBinding>(
        _guard: &'a RwLockReadGuard<'_, ()>,
        space: &'a Lisp2Space<VM>,
    ) -> Self {
        assert_eq!(
            space.common.global_state.gc_status.load(),
            GcStatus::InPause,
            "Lisp2Space forwarding access requires an active STW GC pause"
        );
        Self { extra_spans: &[] }
    }

    #[inline(always)]
    fn active_worker<VM: VMBinding>(
        worker: &'a mut GCWorker<VM>,
        space: &'a Lisp2Space<VM>,
    ) -> Self {
        assert!(
            std::ptr::eq(space.scheduler.as_ref(), worker.scheduler()),
            "GCWorker belongs to a different GCWorkScheduler than Lisp2Space"
        );
        assert_eq!(
            space.common.global_state.gc_status.load(),
            GcStatus::InPause,
            "Lisp2Space forwarding access requires an active STW GC pause"
        );
        let extra_spans = if space.fast_span1.1.load(Ordering::Acquire) == usize::MAX {
            space.worker_cached_spans(worker)
        } else {
            &[]
        };
        Self { extra_spans }
    }

    #[inline(always)]
    fn exclusive_parked<VM: VMBinding>(
        parked: &'a mut WorkersParked<'_>,
        _guard: &'a mut RwLockWriteGuard<'_, ()>,
        space: &'a Lisp2Space<VM>,
    ) -> Self {
        assert!(
            parked.monitor_eq(&space.scheduler.worker_monitor),
            "WorkersParked belongs to a different WorkerMonitor than Lisp2Space"
        );
        assert_eq!(
            space.common.global_state.gc_status.load(),
            GcStatus::InPause,
            "Lisp2Space forwarding access requires an active STW GC pause"
        );
        Self { extra_spans: &[] }
    }
}

pub struct Lisp2Space<VM: VMBinding> {
    common: CommonSpace<VM>,
    pr: MonotonePageResource<VM>,
    scheduler: Arc<GCWorkScheduler<VM>>,
    /// Synchronizes cold external `SFT` queries (`try_read`) against `compact` (`try_write`).
    compact_rwlock: RwLock<()>,
    /// Inline fast-path `(start, end)` bounds for up to two coalesced allocated spans in `self`.
    /// On 64-bit `Map64`, `MonotonePageResource` owns a contiguous chunk slice with at most one
    /// partially-filled `current_chunk`, so sorting and coalescing yields `<= 2` spans.
    /// `fast_span1` is `(usize::MAX, 0)` when `<= 1` span exists, `(s0, s1)` when `2` spans exist,
    /// or `(usize::MAX, usize::MAX)` when `>= 3` spans exist on fragmented 32-bit heaps.
    fast_span0: (AtomicUsize, AtomicUsize),
    fast_span1: (AtomicUsize, AtomicUsize),
    /// Monotonic epoch (`0` when unpopulated) paired with `mapped_spans` when `>= 3` spans exist.
    spans_epoch: AtomicU64,
    /// Sorted, non-overlapping validated `(start, end)` allocated spans when `>= 3` spans exist.
    mapped_spans: RwLock<ValidatedSpans>,
}

const GC_MARK_BIT_MASK: u8 = 1;

/// For each Lisp2 object, we need one extra word for storing forwarding pointer (Lisp-2 implementation).
/// Note that considering the object alignment, we may end up allocating/reserving more than one word per object.
/// See [`Lisp2Space::HEADER_RESERVED_IN_BYTES`].
pub const GC_EXTRA_HEADER_WORD: usize = 1;
const GC_EXTRA_HEADER_BYTES: usize = GC_EXTRA_HEADER_WORD << LOG_BYTES_IN_WORD;
const _: () = assert!(GC_EXTRA_HEADER_BYTES == std::mem::size_of::<AtomicUsize>());

impl<VM: VMBinding> SFT for Lisp2Space<VM> {
    fn name(&self) -> &'static str {
        self.get_name()
    }

    fn get_forwarded_object(&self, object: ObjectReference) -> Option<ObjectReference> {
        if self.common.global_state.gc_status.load() != GcStatus::InPause {
            return None;
        }
        let guard = self
            .compact_rwlock
            .try_read()
            .expect("Cannot inspect Lisp2Space forwarding pointer during compact");
        self.load_forwarding_pointer(object, ForwardingAccess::shared_external(&guard, self))
    }

    fn is_live(&self, object: ObjectReference) -> bool {
        Self::is_marked(object)
    }

    #[cfg(feature = "object_pinning")]
    fn pin_object(&self, _object: ObjectReference) -> bool {
        panic!("Cannot pin/unpin objects of Lisp2Space.")
    }

    #[cfg(feature = "object_pinning")]
    fn unpin_object(&self, _object: ObjectReference) -> bool {
        panic!("Cannot pin/unpin objects of Lisp2Space.")
    }

    #[cfg(feature = "object_pinning")]
    fn is_object_pinned(&self, _object: ObjectReference) -> bool {
        false
    }

    fn is_movable(&self) -> bool {
        true
    }

    fn initialize_object_metadata(&self, object: ObjectReference, _bytes: usize) {
        crate::util::metadata::vo_bit::set_vo_bit(object);
    }

    #[cfg(feature = "sanity")]
    fn is_sane(&self) -> bool {
        true
    }

    #[cfg(feature = "vo_bit")]
    fn is_mmtk_object(&self, addr: Address) -> Option<ObjectReference> {
        crate::util::metadata::vo_bit::is_vo_bit_set_for_addr(addr)
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

    fn sft_trace_object(
        &self,
        _queue: &mut OptionObjectQueue,
        _object: ObjectReference,
        _worker: GCWorkerMutRef,
    ) -> ObjectReference {
        // We should not use trace_object for Lisp2 space.
        // Depending on which trace it is, we should manually call either trace_mark or trace_forward.
        panic!("sft_trace_object() cannot be used with mark compact space")
    }

    fn debug_print_object_info(&self, object: ObjectReference) {
        let forwarded = if self.common.global_state.gc_status.load() == GcStatus::InPause {
            let guard = self
                .compact_rwlock
                .try_read()
                .expect("Cannot inspect Lisp2Space forwarding pointer during compact");
            self.load_forwarding_pointer(object, ForwardingAccess::shared_external(&guard, self))
        } else {
            None
        };
        println!("marked = {}", Lisp2Space::<VM>::is_marked(object));
        println!("head forwarding pointer = {:?}", forwarded);
        self.common.debug_print_object_global_info(object);
    }
}

impl<VM: VMBinding> Space<VM> for Lisp2Space<VM> {
    fn as_space(&self) -> &dyn Space<VM> {
        self
    }

    fn as_sft(&self) -> &(dyn SFT + Sync + 'static) {
        self
    }

    fn get_page_resource(&self) -> &dyn PageResource<VM> {
        &self.pr
    }

    fn maybe_get_page_resource_mut(&mut self) -> Option<&mut dyn PageResource<VM>> {
        Some(&mut self.pr)
    }

    fn common(&self) -> &CommonSpace<VM> {
        &self.common
    }

    fn release_multiple_pages(&mut self, _start: Address) {
        panic!("Lisp2 space only releases pages enmasse")
    }

    fn enumerate_objects(&self, enumerator: &mut dyn ObjectEnumerator) {
        object_enum::enumerate_blocks_from_monotonic_page_resource(enumerator, &self.pr);
    }

    fn clear_side_log_bits(&self) {
        unimplemented!()
    }

    fn set_side_log_bits(&self) {
        unimplemented!()
    }
}

impl<VM: VMBinding> crate::policy::gc_work::PolicyTraceObject<VM> for Lisp2Space<VM> {
    #[inline(always)]
    fn trace_object<Q: ObjectQueue, const KIND: crate::policy::gc_work::TraceKind>(
        &self,
        queue: &mut Q,
        object: ObjectReference,
        _copy: Option<CopySemantics>,
        worker: &mut GCWorker<VM>,
    ) -> ObjectReference {
        debug_assert!(
            KIND != TRACE_KIND_TRANSITIVE_PIN,
            "Lisp2 does not support transitive pin trace."
        );
        if KIND == TRACE_KIND_MARK {
            self.trace_mark_object(queue, object)
        } else if KIND == TRACE_KIND_FORWARD {
            self.trace_forward_object(queue, object, worker)
        } else {
            unreachable!()
        }
    }
    fn may_move_objects<const KIND: crate::policy::gc_work::TraceKind>() -> bool {
        if KIND == TRACE_KIND_MARK {
            false
        } else if KIND == TRACE_KIND_FORWARD {
            true
        } else {
            unreachable!()
        }
    }
}

impl<VM: VMBinding> Lisp2Space<VM> {
    /// We need one extra header word for each object. Considering the alignment requirement, this is
    /// the actual bytes we need to reserve for each allocation.
    pub const HEADER_RESERVED_IN_BYTES: usize = if VM::MAX_ALIGNMENT > GC_EXTRA_HEADER_BYTES {
        VM::MAX_ALIGNMENT
    } else {
        GC_EXTRA_HEADER_BYTES
    }
    .next_power_of_two();

    fn clear_validated_spans(&self) {
        self.spans_epoch.store(0, Ordering::Release);
        self.fast_span1.0.store(usize::MAX, Ordering::Release);
        self.fast_span1.1.store(0, Ordering::Release);
        self.fast_span0.0.store(usize::MAX, Ordering::Release);
        self.fast_span0.1.store(0, Ordering::Release);
        *self.mapped_spans.write().unwrap() = Arc::from([]);
    }

    #[cold]
    #[inline(never)]
    fn worker_cached_spans<'a>(&'a self, worker: &'a mut GCWorker<VM>) -> &'a [(Address, Address)] {
        let epoch = self.spans_epoch.load(Ordering::Acquire);
        if epoch == 0 {
            return &[];
        }
        let needs_refresh = match &worker.lisp2_spans_cache {
            Some((cached_epoch, _)) => *cached_epoch != epoch,
            None => true,
        };
        if needs_refresh {
            let arc = self.mapped_spans.read().unwrap().clone();
            worker.lisp2_spans_cache = Some((epoch, arc));
        }
        &worker.lisp2_spans_cache.as_ref().unwrap().1
    }

    /// Sort `(start, size)` allocated regions in ascending address order and coalesce adjacent
    /// spans (`MonotonePageResource::iterate_allocated_regions` yields discontiguous chunks in
    /// newest-to-oldest descending order).
    fn sort_and_coalesce_spans(
        regions: impl IntoIterator<Item = (Address, usize)>,
    ) -> Vec<(Address, Address)> {
        let mut raw_spans: Vec<(Address, Address)> = regions
            .into_iter()
            .filter_map(|(start, size)| (size > 0).then_some((start, start + size)))
            .collect();
        raw_spans.sort_unstable_by_key(|&(s0, _)| s0);
        let mut spans: Vec<(Address, Address)> = Vec::with_capacity(raw_spans.len());
        for (start, end) in raw_spans {
            if let Some(last) = spans.last_mut() {
                assert!(
                    last.1 <= start,
                    "Overlapping Lisp2Space allocated regions {:?} and ({start}, {end})",
                    last
                );
                if last.1 == start {
                    last.1 = end;
                    continue;
                }
            }
            spans.push((start, end));
        }
        spans
    }

    /// Coalesce allocated `MonotonePageResource` regions and validate once per chunk and `MMAPPER`
    /// grain that every coalesced span `[s0, s1)` belongs to `self` and resides on mapped OS pages.
    fn populate_validated_spans(&self) {
        let spans = Self::sort_and_coalesce_spans(self.pr.iterate_allocated_regions());
        if spans.is_empty() {
            self.clear_validated_spans();
            return;
        }

        let grain = MMAPPER.granularity();
        for &(s0, s1) in &spans {
            assert!(
                s0.as_usize() > GC_EXTRA_HEADER_BYTES && s0 < s1,
                "Invalid Lisp2Space allocated span [{s0}, {s1})"
            );
            let last_byte = s1 - 1usize;

            let first_chunk = s0.align_down(BYTES_IN_CHUNK);
            let last_chunk = last_byte.align_down(BYTES_IN_CHUNK);
            let mut cur_chunk = first_chunk;
            loop {
                assert!(
                    self.address_in_space(cur_chunk.max(s0)),
                    "Lisp2Space allocated span [{s0}, {s1}) falls outside space at {cur_chunk}"
                );
                if cur_chunk == last_chunk {
                    break;
                }
                cur_chunk += BYTES_IN_CHUNK;
            }
            assert!(
                self.address_in_space(last_byte),
                "Lisp2Space allocated span tail {last_byte} falls outside space"
            );

            let first_grain = s0.align_down(grain);
            let last_grain = last_byte.align_down(grain);
            let mut cur_grain = first_grain;
            loop {
                assert!(
                    MMAPPER.is_mapped_address(cur_grain.max(s0)),
                    "Unmapped OS grain {cur_grain} inside Lisp2Space allocated span [{s0}, {s1})"
                );
                if cur_grain == last_grain {
                    break;
                }
                cur_grain += grain;
            }
        }

        let (s0_0, s0_1) = spans[0];
        assert!(
            s0_1.as_usize() < usize::MAX,
            "Lisp2Space span upper bound {s0_1} collides with multi-span sentinel"
        );
        if self.fast_span0.1.load(Ordering::Relaxed) != 0
            || self.fast_span1.1.load(Ordering::Relaxed) != 0
        {
            self.clear_validated_spans();
        }

        match spans.len() {
            1 => {
                self.fast_span0.0.store(s0_0.as_usize(), Ordering::Relaxed);
                self.fast_span0.1.store(s0_1.as_usize(), Ordering::Release);
            }
            2 => {
                let (s1_0, s1_1) = spans[1];
                assert!(
                    s1_1.as_usize() < usize::MAX,
                    "Lisp2Space span upper bound {s1_1} collides with multi-span sentinel"
                );
                self.fast_span0.0.store(s0_0.as_usize(), Ordering::Relaxed);
                self.fast_span0.1.store(s0_1.as_usize(), Ordering::Release);
                self.fast_span1.0.store(s1_0.as_usize(), Ordering::Relaxed);
                self.fast_span1.1.store(s1_1.as_usize(), Ordering::Release);
            }
            _ => {
                let epoch = NEXT_LISP2_EPOCH.fetch_add(1, Ordering::Relaxed);
                assert_ne!(epoch, 0, "Lisp2Space epoch counter wrapped");
                *self.mapped_spans.write().unwrap() = Arc::from(spans.into_boxed_slice());
                self.spans_epoch.store(epoch, Ordering::Release);
                self.fast_span0.0.store(s0_0.as_usize(), Ordering::Relaxed);
                self.fast_span0.1.store(s0_1.as_usize(), Ordering::Release);
                self.fast_span1.0.store(usize::MAX, Ordering::Relaxed);
                self.fast_span1.1.store(usize::MAX, Ordering::Release);
            }
        }
    }

    #[inline(always)]
    fn is_span_mapped_in_space(
        &self,
        lo: Address,
        hi_inclusive: Address,
        access: ForwardingAccess<'_>,
    ) -> bool {
        debug_assert!(lo <= hi_inclusive);
        let end0 = self.fast_span0.1.load(Ordering::Acquire);
        let start0 = self.fast_span0.0.load(Ordering::Relaxed);
        if lo.as_usize() >= start0 && hi_inclusive.as_usize() < end0 {
            return true;
        }
        let end1 = self.fast_span1.1.load(Ordering::Acquire);
        if end1 != 0 && end1 != usize::MAX {
            let start1 = self.fast_span1.0.load(Ordering::Relaxed);
            if lo.as_usize() >= start1 && hi_inclusive.as_usize() < end1 {
                return true;
            }
        }
        self.is_span_mapped_in_space_slow(lo, hi_inclusive, end1, access.extra_spans)
    }

    #[cold]
    #[inline(never)]
    fn is_span_mapped_in_space_slow(
        &self,
        lo: Address,
        hi_inclusive: Address,
        end1: usize,
        extra_spans: &[(Address, Address)],
    ) -> bool {
        if !extra_spans.is_empty() {
            let idx = extra_spans.partition_point(|&(s0, _)| s0 <= lo);
            if idx > 0 && hi_inclusive < extra_spans[idx - 1].1 {
                return true;
            }
        } else if end1 == usize::MAX {
            let spans = self.mapped_spans.read().unwrap();
            let idx = spans.partition_point(|&(s0, _)| s0 <= lo);
            if idx > 0 && hi_inclusive < spans[idx - 1].1 {
                return true;
            }
        }
        // Fallback when queried during an STW pause before `CalculateForwarding` populates spans.
        if !self.address_in_space(lo) || !self.address_in_space(hi_inclusive) {
            return false;
        }
        let first_page = lo.align_down(BYTES_IN_PAGE);
        let last_page = hi_inclusive.align_down(BYTES_IN_PAGE);
        let mut page = first_page;
        loop {
            if !page.max(lo).is_mapped() {
                return false;
            }
            if page == last_page {
                return hi_inclusive.is_mapped();
            }
            page += BYTES_IN_PAGE;
        }
    }

    // The following are helpers for manipulating the header forwarding pointer.
    // For each allocation request, `Lisp2Allocator` reserves `HEADER_RESERVED_IN_BYTES` extra
    // bytes before `object.to_object_start::<VM>()`, placing the forwarding pointer word at
    // `object.to_object_start::<VM>() - GC_EXTRA_HEADER_BYTES`.

    #[inline(always)]
    fn forwarding_slot<'a>(
        &'a self,
        object: ObjectReference,
        require_vo_bit: bool,
        access: ForwardingAccess<'a>,
    ) -> &'a AtomicUsize {
        let raw = object.to_raw_address();
        let obj_start = object.to_object_start::<VM>();
        assert!(
            raw.is_aligned_to(ObjectReference::ALIGNMENT)
                && (!require_vo_bit || vo_bit::is_vo_bit_set(object))
                && obj_start.as_usize() > GC_EXTRA_HEADER_BYTES,
            "Object {object} is not a valid object in Lisp2Space",
        );
        let fwd_addr = obj_start - GC_EXTRA_HEADER_BYTES;
        let lo = fwd_addr.min(raw);
        let hi_inclusive = (obj_start - 1usize).max(raw);
        assert!(
            fwd_addr.is_aligned_to(std::mem::align_of::<AtomicUsize>())
                && self.is_span_mapped_in_space(lo, hi_inclusive, access),
            "Object {object} forwarding header word {fwd_addr} is invalid or unmapped in Lisp2Space",
        );
        // SAFETY:
        // 1. `obj_start.as_usize() > GC_EXTRA_HEADER_BYTES` guarantees `!fwd_addr.is_zero()`,
        //    `access: ForwardingAccess<'a>` verified `GcStatus::InPause` (mutators stopped; no
        //    concurrent non-atomic bump-page zeroing or alignment-gap fills), and the assertions
        //    above verified `fwd_addr` is aligned to `AtomicUsize` and `[lo, hi_inclusive]`
        //    (covering both `raw` and all bytes of `AtomicUsize` at `fwd_addr`) belongs to `self`
        //    and resides on mapped OS pages (`MMAPPER` never unmaps zero-initialized pages).
        // 2. Shared callers (`active_worker`, `shared_external`) pass `require_vo_bit: true`,
        //    proving `object` is an allocated object whose `[fwd_addr, obj_start)` header word is
        //    disjoint from every object payload, while `access` excludes `compact`. In `compact`,
        //    `clear_destination_forwarding_pointer` holds `exclusive_parked` (`&'a mut`), so no
        //    concurrent threads access `self` and `'a` ends before `VMObjectModel::copy_to` runs.
        unsafe { &*fwd_addr.to_ptr::<AtomicUsize>() }
    }

    #[inline(always)]
    fn load_forwarding_pointer(
        &self,
        object: ObjectReference,
        access: ForwardingAccess<'_>,
    ) -> Option<ObjectReference> {
        let raw = self
            .forwarding_slot(object, true, access)
            .load(Ordering::Relaxed);
        if raw == 0 {
            return None;
        }
        // SAFETY: `Address::from_usize` converts the raw forwarding-pointer word (`0` handled
        // above, or a valid destination `ObjectReference` raw address written by
        // `store_forwarding_pointer`) without dereferencing it.
        let addr = unsafe { Address::from_usize(raw) };
        assert!(
            addr.is_aligned_to(ObjectReference::ALIGNMENT)
                && self.is_span_mapped_in_space(addr, addr, access),
            "Corrupted Lisp2Space forwarding pointer {addr} for {object}",
        );
        ObjectReference::from_raw_address(addr)
    }

    #[inline(always)]
    fn store_forwarding_pointer(
        &self,
        object: ObjectReference,
        forwarding_pointer: ObjectReference,
        access: ForwardingAccess<'_>,
    ) {
        self.forwarding_slot(object, true, access).store(
            forwarding_pointer.to_raw_address().as_usize(),
            Ordering::Relaxed,
        );
    }

    #[inline(always)]
    fn clear_destination_forwarding_pointer(
        &self,
        new_object: ObjectReference,
        access: ForwardingAccess<'_>,
    ) {
        self.forwarding_slot(new_object, false, access)
            .store(0, Ordering::Relaxed);
    }

    pub fn new(args: crate::policy::space::PlanCreateSpaceArgs<VM>) -> Self {
        let vm_map = args.vm_map;
        let is_discontiguous = args.vmrequest.is_discontiguous();
        let scheduler = args.scheduler.clone();
        let local_specs = extract_side_metadata(&[*VM::VMObjectModel::LOCAL_MARK_BIT_SPEC]);
        let common = CommonSpace::new(args.into_policy_args(true, false, local_specs));
        Lisp2Space {
            pr: if is_discontiguous {
                MonotonePageResource::new_discontiguous(vm_map)
            } else {
                MonotonePageResource::new_contiguous(common.start, common.extent, vm_map)
            },
            common,
            scheduler,
            compact_rwlock: RwLock::new(()),
            fast_span0: (AtomicUsize::new(usize::MAX), AtomicUsize::new(0)),
            fast_span1: (AtomicUsize::new(usize::MAX), AtomicUsize::new(0)),
            spans_epoch: AtomicU64::new(0),
            mapped_spans: RwLock::new(Arc::from([])),
        }
    }

    pub fn prepare(&self) {
        self.clear_validated_spans();
    }

    pub fn release(&self) {
        self.clear_validated_spans();
    }

    #[inline(always)]
    pub fn trace_mark_object<Q: ObjectQueue>(
        &self,
        queue: &mut Q,
        object: ObjectReference,
    ) -> ObjectReference {
        debug_assert!(
            crate::util::metadata::vo_bit::is_vo_bit_set(object),
            "{:x}: VO bit not set",
            object
        );
        if Lisp2Space::<VM>::test_and_mark(object) {
            queue.enqueue(object);
        }
        object
    }

    #[inline(always)]
    pub(crate) fn trace_forward_object<Q: ObjectQueue>(
        &self,
        queue: &mut Q,
        object: ObjectReference,
        worker: &mut GCWorker<VM>,
    ) -> ObjectReference {
        let access = ForwardingAccess::active_worker(worker, self);
        let forwarded = self
            .load_forwarding_pointer(object, access)
            .unwrap_or_else(|| panic!("Object {object} does not have a forwarding pointer"));
        // From this stage onwards, mark bit is no longer needed; reuse it for the forwarding visit.
        if Lisp2Space::<VM>::test_and_clear_mark(object) {
            queue.enqueue(object);
        }
        forwarded
    }

    #[inline(always)]
    pub fn test_and_mark(object: ObjectReference) -> bool {
        loop {
            let old_value = VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.load_atomic::<VM, u8>(
                object,
                None,
                Ordering::SeqCst,
            );
            let mark_bit = old_value & GC_MARK_BIT_MASK;
            if mark_bit != 0 {
                return false;
            }
            if VM::VMObjectModel::LOCAL_MARK_BIT_SPEC
                .compare_exchange_metadata::<VM, u8>(
                    object,
                    old_value,
                    1,
                    None,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
            {
                break;
            }
        }
        true
    }

    #[inline(always)]
    pub fn test_and_clear_mark(object: ObjectReference) -> bool {
        loop {
            let old_value = VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.load_atomic::<VM, u8>(
                object,
                None,
                Ordering::SeqCst,
            );
            let mark_bit = old_value & GC_MARK_BIT_MASK;
            if mark_bit == 0 {
                return false;
            }

            if VM::VMObjectModel::LOCAL_MARK_BIT_SPEC
                .compare_exchange_metadata::<VM, u8>(
                    object,
                    old_value,
                    0,
                    None,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
            {
                break;
            }
        }
        true
    }

    #[inline(always)]
    pub fn is_marked(object: ObjectReference) -> bool {
        let old_value = VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.load_atomic::<VM, u8>(
            object,
            None,
            Ordering::SeqCst,
        );
        let mark_bit = old_value & GC_MARK_BIT_MASK;
        mark_bit != 0
    }

    #[inline(always)]
    fn to_be_compacted(object: &ObjectReference) -> bool {
        Self::is_marked(*object)
    }

    /// Linear scan all the live objects in the given memory region
    fn linear_scan_objects(&self, range: Range<Address>) -> impl Iterator<Item = ObjectReference> {
        crate::util::linear_scan::ObjectIterator::<VM, Lisp2ObjectSize<VM>, true>::new(
            range.start,
            range.end,
        )
    }

    pub(crate) fn calculate_forwarding_pointer(&self, worker: &mut GCWorker<VM>) {
        self.populate_validated_spans();
        let access = ForwardingAccess::active_worker(worker, self);
        let mut to_iter = self.pr.iterate_allocated_regions();
        let Some((mut to_cursor, mut to_size)) = to_iter.next() else {
            return;
        };
        let mut to_end = to_cursor + to_size;
        for (from_start, size) in self.pr.iterate_allocated_regions() {
            let from_end = from_start + size;
            for obj in self
                .linear_scan_objects(from_start..from_end)
                .filter(Self::to_be_compacted)
            {
                let copied_size =
                    VM::VMObjectModel::get_size_when_copied(obj) + Self::HEADER_RESERVED_IN_BYTES;
                let align = VM::VMObjectModel::get_align_when_copied(obj);
                let offset = VM::VMObjectModel::get_align_offset_when_copied(obj);
                to_cursor = align_allocation_no_fill::<VM>(to_cursor, align, offset);
                if to_cursor + copied_size > to_end {
                    (to_cursor, to_size) = to_iter.next().unwrap();
                    to_end = to_cursor + to_size;
                    to_cursor = align_allocation_no_fill::<VM>(to_cursor, align, offset);
                    debug_assert!(to_cursor + copied_size <= to_end);
                }
                let new_obj = VM::VMObjectModel::get_reference_when_copied_to(
                    obj,
                    to_cursor + Self::HEADER_RESERVED_IN_BYTES,
                );
                self.store_forwarding_pointer(obj, new_obj, access);
                trace!(
                    "Calculate forward: {} (size when copied = {}) ~> {} (size = {})",
                    obj,
                    VM::VMObjectModel::get_size_when_copied(obj),
                    to_cursor,
                    copied_size
                );
                to_cursor += copied_size;
            }
        }
    }

    pub(crate) fn compact(&self, parked: &mut WorkersParked<'_>) {
        assert!(
            parked.monitor_eq(&self.scheduler.worker_monitor),
            "WorkersParked belongs to a different WorkerMonitor than Lisp2Space"
        );
        assert_eq!(
            self.common.global_state.gc_status.load(),
            GcStatus::InPause,
            "Lisp2Space::compact requires an active STW GC pause"
        );
        let mut guard = self
            .compact_rwlock
            .try_write()
            .expect("Cannot run Lisp2Space::compact concurrently with forwarding-pointer access");
        let mut to = Address::ZERO;
        for (from_start, size) in self.pr.iterate_allocated_regions() {
            let from_end = from_start + size;
            for obj in self.linear_scan_objects(from_start..from_end) {
                let copied_size = VM::VMObjectModel::get_size_when_copied(obj);
                let maybe_forwarding_pointer = self.load_forwarding_pointer(
                    obj,
                    ForwardingAccess::exclusive_parked(parked, &mut guard, self),
                );
                vo_bit::unset_vo_bit(obj);

                if let Some(new_object) = maybe_forwarding_pointer {
                    trace!("Compact {} to {}", obj, new_object);
                    self.clear_destination_forwarding_pointer(
                        new_object,
                        ForwardingAccess::exclusive_parked(parked, &mut guard, self),
                    );

                    trace!(" copy from {} to {}", obj, new_object);
                    let end_of_new_object =
                        VM::VMObjectModel::copy_to(obj, new_object, Address::ZERO);
                    vo_bit::set_vo_bit(new_object);
                    to = new_object.to_object_start::<VM>() + copied_size;
                    debug_assert_eq!(end_of_new_object, to);
                } else {
                    trace!("Skipping dead object {}", obj);
                }
            }
        }

        debug!("Compact end: to = {}", to);
        self.pr.reset_cursor(to);
    }
}

struct Lisp2ObjectSize<VM>(PhantomData<VM>);
impl<VM: VMBinding> crate::util::linear_scan::LinearScanObjectSize for Lisp2ObjectSize<VM> {
    #[inline(always)]
    fn size(object: ObjectReference) -> usize {
        VM::VMObjectModel::get_current_size(object)
    }
}

#[cfg(all(test, feature = "mock_test"))]
mod tests {
    use super::*;
    use crate::util::test_util::mock_vm::MockVM;

    #[test]
    fn sort_and_coalesce_spans_merges_descending_discontiguous_chunks() {
        let backing = Box::new([0u64; 1536]); // 12 KiB stack/heap allocation for valid addresses
        let base = Address::from_ref(&backing[0]);
        // Newest-to-oldest order yielded by `MonotonePageResource::iterate_allocated_regions`:
        // `[base + 8K, base + 10K)`, `[base + 4K, base + 8K)`, `[base, base + 4K)`.
        let spans = Lisp2Space::<MockVM>::sort_and_coalesce_spans([
            (base + 8192usize, 2048),
            (base + 4096usize, 4096),
            (base, 4096),
        ]);
        assert_eq!(spans, vec![(base, base + 10240usize)]);

        // Partially filled older `current_chunk` + full newer chunk yields 2 sorted spans.
        let two_spans = Lisp2Space::<MockVM>::sort_and_coalesce_spans([
            (base + 8192usize, 4096),
            (base + 4096usize, 1024),
        ]);
        assert_eq!(
            two_spans,
            vec![
                (base + 4096usize, base + 5120usize),
                (base + 8192usize, base + 12288usize),
            ]
        );
    }
}
