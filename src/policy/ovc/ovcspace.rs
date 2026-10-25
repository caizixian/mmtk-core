#![deny(unsafe_op_in_unsafe_fn)]

use crate::plan::tracing::OptionObjectQueue;
use crate::policy::gc_work::{TraceKind, TRACE_KIND_TRANSITIVE_PIN};
use crate::policy::largeobjectspace::LargeObjectSpace;
use crate::policy::ovc::forwarding;
use crate::policy::sft::{GCWorkerMutRef, SFT};
use crate::policy::space::{CommonSpace, Space};
use crate::scheduler::{GCWork, GCWorkScheduler, GCWorker, WorkBucketStage};
use crate::util::constants::BYTES_IN_WORD;
use crate::util::copy::CopySemantics;
use crate::util::heap::regionpageresource::AllocatedRegion;
use crate::util::heap::{PageResource, RegionPageResource};
use crate::util::linear_scan::Region;
use crate::util::metadata::extract_side_metadata;
#[cfg(feature = "vo_bit")]
use crate::util::metadata::vo_bit;
use crate::util::metadata::MetadataSpec;
use crate::util::object_enum::{self, ObjectEnumerator};
use crate::util::{Address, ObjectReference};
use crate::vm::slot::Slot;
use crate::MMTK;
use crate::{vm::*, ObjectQueue};
use atomic::Ordering;
use std::sync::{Arc, RwLock};

pub(crate) const TRACE_KIND_MARK: TraceKind = 0;
pub(crate) const TRACE_KIND_FORWARD_ROOT: TraceKind = 1;

/// [`OVCSpace`] is a stop-the-world implementation of
/// the Compressor, as described in Kermany and Petrank,
/// [The Compressor: concurrent, incremental, and parallel compaction](https://dl.acm.org/doi/10.1145/1133255.1134023).
///
/// [`OVCSpace`] makes two main diversions from the paper
/// (aside from [`OVCSpace`] being stop-the-world):
/// - The heap is structured into regions ([`forwarding::OVCRegion`])
///   which the collector compacts separately.
/// - The collector compacts each region in-place, instead of using two virtual
///   spaces as in Kermany and Petrank. The virtual spaces side-step a race which
///   would occur if multiple threads attempted to compact one heap in place: one thread
///   could move an object to the location of another object which has yet to be moved by
///   another thread. Kermany and Petrank move objects between from- and to- virtual spaces,
///   preventing the old objects from being overwritten. (They reclaim memory by unmapping
///   pages of the from-virtual space after moving all objects out of said pages.)
///   We instead side-step this race by assigning only a single thread to each region, and
///   running multiple single-threaded OVCs at once.
pub struct OVCSpace<VM: VMBinding> {
    common: CommonSpace<VM>,
    pr: RegionPageResource<VM, forwarding::OVCRegion>,
    forwarding: forwarding::ForwardingMetadata<VM>,
    scheduler: Arc<GCWorkScheduler<VM>>,
    /// Synchronizes non-atomic `MARK_SPEC.bzero_metadata` in `prepare()` against metadata readers
    /// (`get_forwarded_object`, `is_live`, `debug_print_object_info`, `trace_forward_root`,
    /// offset-vector calculation, and compaction).
    prepare_rwlock: RwLock<()>,
}

impl<VM: VMBinding> SFT for OVCSpace<VM> {
    fn name(&self) -> &'static str {
        self.get_name()
    }

    fn get_forwarded_object(&self, object: ObjectReference) -> Option<ObjectReference> {
        let _prepare_read = self
            .prepare_rwlock
            .try_read()
            .expect("Cannot inspect OVCSpace forwarding pointer during prepare");
        let regions = self.forwarding.calculated_regions_if_ready()?;
        Self::assert_in_calculated_regions(&regions, self.in_space(object), object);
        // SAFETY: `calculated_regions_if_ready()` holds the read guard pinning `calculated == true`
        // and `regions`, `assert_in_calculated_regions` verified `object` is word-aligned inside a
        // mapped, calculated `[region.start(), pre_compact_cursor)` range of `self`, and
        // `_prepare_read` prevents concurrent non-atomic `prepare()` (`bzero_metadata`).
        Some(unsafe { self.forward(object, false) })
    }

    fn is_live(&self, object: ObjectReference) -> bool {
        let _prepare_read = self
            .prepare_rwlock
            .try_read()
            .expect("Cannot inspect OVCSpace mark bit during prepare");
        self.assert_mark_mapped_in_space(object);
        Self::is_marked(object)
    }

    #[cfg(feature = "object_pinning")]
    fn pin_object(&self, _object: ObjectReference) -> bool {
        panic!("Cannot pin/unpin objects of OVCSpace.")
    }

    #[cfg(feature = "object_pinning")]
    fn unpin_object(&self, _object: ObjectReference) -> bool {
        panic!("Cannot pin/unpin objects of OVCSpace.")
    }

    #[cfg(feature = "object_pinning")]
    fn is_object_pinned(&self, _object: ObjectReference) -> bool {
        false
    }

    fn is_movable(&self) -> bool {
        true
    }

    fn initialize_object_metadata(&self, _object: ObjectReference, _bytes: usize) {
        #[cfg(feature = "vo_bit")]
        crate::util::metadata::vo_bit::set_vo_bit(_object);
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
        // We should not use trace_object for OVC space.
        // Depending on which trace it is, we should manually call either trace_mark or trace_forward.
        panic!("sft_trace_object() cannot be used with OVCSpace")
    }

    fn debug_print_object_info(&self, object: ObjectReference) {
        let forwarded = {
            let _prepare_read = self
                .prepare_rwlock
                .try_read()
                .expect("Cannot inspect OVCSpace metadata during prepare");
            self.assert_mark_mapped_in_space(object);
            println!("marked = {}", OVCSpace::<VM>::is_marked(object));
            if let Some(regions) = self.forwarding.calculated_regions_if_ready() {
                Self::assert_in_calculated_regions(&regions, self.in_space(object), object);
                // SAFETY: `regions` pins `calculated == true`, `assert_in_calculated_regions`
                // verified `object` lies in a mapped, calculated `[region.start(), pre_compact_cursor)`
                // range of `self`, and `_prepare_read` excludes `prepare()`.
                Some(unsafe { self.forward(object, false) })
            } else {
                None
            }
        };
        println!("forwarding = {:?}", forwarded);
        self.common.debug_print_object_global_info(object);
    }
}

impl<VM: VMBinding> Space<VM> for OVCSpace<VM> {
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

    fn initialize_sft(&self, sft_map: &mut dyn crate::policy::sft_map::SFTMap) {
        self.common().initialize_sft(self.as_sft(), sft_map)
    }

    fn release_multiple_pages(&mut self, _start: Address) {
        panic!("ovc space only releases pages enmasse")
    }

    fn enumerate_objects(&self, enumerator: &mut dyn ObjectEnumerator) {
        self.pr.enumerate(enumerator);
    }

    fn clear_side_log_bits(&self) {
        unimplemented!()
    }

    fn set_side_log_bits(&self) {
        unimplemented!()
    }
}

impl<VM: VMBinding> crate::policy::gc_work::PolicyTraceObject<VM> for OVCSpace<VM> {
    fn trace_object<Q: ObjectQueue, const KIND: crate::policy::gc_work::TraceKind>(
        &self,
        queue: &mut Q,
        object: ObjectReference,
        _copy: Option<CopySemantics>,
        _worker: &mut GCWorker<VM>,
    ) -> ObjectReference {
        debug_assert!(
            KIND != TRACE_KIND_TRANSITIVE_PIN,
            "OVC does not support transitive pin trace."
        );
        if KIND == TRACE_KIND_MARK {
            self.trace_mark_object(queue, object)
        } else if KIND == TRACE_KIND_FORWARD_ROOT {
            self.trace_forward_root(queue, object)
        } else {
            unreachable!()
        }
    }
    fn may_move_objects<const KIND: crate::policy::gc_work::TraceKind>() -> bool {
        if KIND == TRACE_KIND_MARK {
            false
        } else if KIND == TRACE_KIND_FORWARD_ROOT {
            true
        } else {
            unreachable!()
        }
    }
}

impl<VM: VMBinding> OVCSpace<VM> {
    pub(crate) fn new(args: crate::policy::space::PlanCreateSpaceArgs<VM>) -> Self {
        let vm_map = args.vm_map;
        assert!(
            VM::VMObjectModel::UNIFIED_OBJECT_REFERENCE_ADDRESS,
            "OVC requires a unified object reference address model"
        );
        let local_specs = extract_side_metadata(&[
            MetadataSpec::OnSide(forwarding::MARK_SPEC),
            MetadataSpec::OnSide(forwarding::OFFSET_VECTOR_SPEC),
        ]);
        let is_discontiguous = args.vmrequest.is_discontiguous();
        let scheduler = args.scheduler.clone();
        let common = CommonSpace::new(args.into_policy_args(true, false, local_specs));
        OVCSpace {
            pr: if is_discontiguous {
                RegionPageResource::new_discontiguous(vm_map)
            } else {
                RegionPageResource::new_contiguous(common.start, common.extent, vm_map)
            },
            forwarding: forwarding::ForwardingMetadata::new(),
            common,
            scheduler,
            prepare_rwlock: RwLock::new(()),
        }
    }

    /// Verify that `object` belongs to `self`, is word- and `ObjectReference`-aligned, and lies
    /// within a mapped, offset-vector-calculated `[region.start(), pre_compact_cursor)` range.
    fn assert_in_calculated_regions(
        regions: &[(forwarding::OVCRegion, Address)],
        in_space: bool,
        object: ObjectReference,
    ) {
        let addr = object.to_raw_address();
        let target_region = forwarding::OVCRegion::from_unaligned_address(addr);
        let valid = in_space
            && addr.is_aligned_to(ObjectReference::ALIGNMENT)
            && addr.is_aligned_to(BYTES_IN_WORD)
            && regions
                .iter()
                .any(|&(region, cursor)| region == target_region && addr < cursor);
        assert!(
            valid,
            "Object {object} is not within a mapped, calculated [region.start(), cursor) range of OVCSpace"
        );
    }

    /// Verify that `object` belongs to `self` and resides in a mapped prefix `[region.start(), bound)`
    /// of an allocated `OVCRegion` (either the active pre-compaction `calculated_regions` cursor
    /// or `self.pr`'s current `AllocatedRegion::cursor`).
    fn assert_mark_mapped_in_space(&self, object: ObjectReference) {
        let addr = object.to_raw_address();
        let target_region = forwarding::OVCRegion::from_unaligned_address(addr);
        let valid = self.in_space(object)
            && addr.is_aligned_to(ObjectReference::ALIGNMENT)
            && addr.is_aligned_to(BYTES_IN_WORD)
            && (self
                .forwarding
                .calculated_regions_if_ready()
                .is_some_and(|regions| {
                    regions
                        .iter()
                        .any(|&(region, cursor)| region == target_region && addr < cursor)
                })
                || self.pr.with_regions(&mut |regions| {
                    regions
                        .iter()
                        .any(|r| r.region == target_region && addr < r.cursor())
                }));
        assert!(
            valid,
            "Object {object} is not in a mapped allocated OVCSpace region"
        );
    }

    pub(crate) fn prepare(&self) {
        let _prepare_write = self
            .prepare_rwlock
            .try_write()
            .expect("Cannot run OVCSpace::prepare concurrently with OVCSpace metadata access");
        self.forwarding.reset();
        self.pr
            .enumerate_regions(&mut |r: &AllocatedRegion<forwarding::OVCRegion>| {
                let start = r.region.start();
                let cursor = r.cursor();
                if cursor > start {
                    forwarding::MARK_SPEC.bzero_metadata(start, cursor - start);
                }
            });
    }

    pub(crate) fn release(&self) {
        self.forwarding.release();
    }

    fn trace_mark_object<Q: ObjectQueue>(
        &self,
        queue: &mut Q,
        object: ObjectReference,
    ) -> ObjectReference {
        #[cfg(feature = "vo_bit")]
        debug_assert!(
            crate::util::metadata::vo_bit::is_vo_bit_set(object),
            "{:x}: VO bit not set",
            object
        );
        if OVCSpace::<VM>::test_and_mark(object) {
            queue.enqueue(object);
            self.forwarding.mark_last_word_of_object(object);
        }
        object
    }

    fn trace_forward_root<Q: ObjectQueue>(
        &self,
        _queue: &mut Q,
        object: ObjectReference,
    ) -> ObjectReference {
        if !self.in_space(object) {
            return object;
        }
        let _prepare_read = self
            .prepare_rwlock
            .try_read()
            .expect("Cannot forward OVCSpace root during prepare");
        let regions = self
            .forwarding
            .calculated_regions_if_ready()
            .expect("OVCSpace::trace_forward_root called before offset vector calculation");
        Self::assert_in_calculated_regions(&regions, true, object);
        // SAFETY: `regions` pins `calculated == true`, `assert_in_calculated_regions` verified
        // `object` is word-aligned inside a mapped, calculated `[region.start(), pre_compact_cursor)`
        // range of `self`, and `_prepare_read` prevents concurrent non-atomic `prepare()`.
        unsafe { self.forward(object, true) }
    }

    fn test_and_mark(object: ObjectReference) -> bool {
        forwarding::MARK_SPEC
            .fetch_update_atomic::<u8, _>(
                object.to_raw_address(),
                Ordering::SeqCst,
                Ordering::Relaxed,
                |v| {
                    if v == 0 {
                        Some(1)
                    } else {
                        None
                    }
                },
            )
            .is_ok()
    }

    fn is_marked(object: ObjectReference) -> bool {
        let mark_bit =
            forwarding::MARK_SPEC.load_atomic::<u8>(object.to_raw_address(), Ordering::SeqCst);
        mark_bit == 1
    }

    fn generate_tasks(
        &self,
        f: &mut impl FnMut(&AllocatedRegion<forwarding::OVCRegion>, usize) -> Box<dyn GCWork<VM>>,
    ) -> Vec<Box<dyn GCWork<VM>>> {
        let mut packets = vec![];
        let mut index = 0;
        self.pr.enumerate_regions(&mut |r| {
            packets.push(f(r, index));
            index += 1;
        });
        packets
    }

    pub(crate) fn add_offset_vector_tasks(&'static self) {
        let _prepare_read = self
            .prepare_rwlock
            .try_read()
            .expect("Cannot schedule OVC offset vector calculation during prepare");
        let mut calculated_regions = Vec::new();
        let offset_vector_packets: Vec<Box<dyn GCWork<VM>>> = self.generate_tasks(&mut |r, _| {
            let cursor = r.cursor();
            calculated_regions.push((r.region, cursor));
            // SAFETY: `r` comes from `self.pr.enumerate_regions`, so `[r.region.start(), cursor)`
            // was acquired via `Space::acquire` with `MARK_SPEC` and `OFFSET_VECTOR_SPEC` mapped
            // and initialized, and `cursor` is page-aligned (`>= Block::BYTES`).
            Box::new(unsafe { CalculateOffsetVector::<VM>::new(self, r.region, cursor) })
        });
        self.forwarding.begin_calculate(calculated_regions);
        self.scheduler.work_buckets[WorkBucketStage::CalculateForwarding]
            .bulk_add(offset_vector_packets);
    }

    /// # Safety
    /// Same preconditions as [`forwarding::ForwardingMetadata::calculate_offset_vector`], paired
    /// with a prior [`forwarding::ForwardingMetadata::begin_calculate`] count.
    unsafe fn calculate_offset_vector_for_region(
        &self,
        region: forwarding::OVCRegion,
        cursor: Address,
    ) {
        let _prepare_read = self
            .prepare_rwlock
            .try_read()
            .expect("Cannot calculate OVC offset vector during prepare");
        // SAFETY: Caller guarantees `region` is an allocated `OVCSpace` region with mapped,
        // initialized `MARK_SPEC` / `OFFSET_VECTOR_SPEC` and `Block`-aligned `cursor`, and
        // `_prepare_read` excludes concurrent non-atomic `prepare()` (`bzero_metadata`).
        unsafe {
            self.forwarding.calculate_offset_vector(region, cursor);
        }
        self.forwarding.finish_region_calculation();
    }

    /// Return `object`'s forwarded reference if `self.in_space(object)`, or `object` unchanged.
    ///
    /// # Safety
    /// * Whenever `self.in_space(object)` is `true`, `object` must be word-aligned inside a mapped,
    ///   calculated `[region.start(), pre_compact_cursor)` range of `self` (so `MARK_SPEC` and
    ///   `OFFSET_VECTOR_SPEC` are mapped and initialized across
    ///   `Block::from_unaligned_address(object.to_raw_address())`),
    ///   `self.forwarding.has_calculated_forwarding_addresses()` must be `true`, and no thread may
    ///   concurrently execute non-atomic `OVCSpace::prepare` (`MARK_SPEC.bzero_metadata`).
    #[inline]
    unsafe fn forward(&self, object: ObjectReference, _vo_bit_valid: bool) -> ObjectReference {
        if !self.in_space(object) {
            return object;
        }
        assert!(
            self.forwarding.has_calculated_forwarding_addresses(),
            "OVCSpace::forward called before offset vector calculation"
        );
        // We can't expect the VO bit to be valid whilst compacting the heap.
        // If we are fixing a reference to an object which was moved before the referent,
        // the relevant VO bit will have been cleared, and this assertion would fail.
        // Thus we can only ever expect the VO bit to be valid whilst fixing the roots.
        #[cfg(feature = "vo_bit")]
        if _vo_bit_valid {
            debug_assert!(
                crate::util::metadata::vo_bit::is_vo_bit_set(object),
                "{:x}: VO bit not set",
                object
            );
        }
        // SAFETY: Caller guarantees `object` is word-aligned in a mapped, calculated
        // `[region.start(), pre_compact_cursor)` range with mapped, initialized `MARK_SPEC` and
        // `OFFSET_VECTOR_SPEC` and no concurrent non-atomic `prepare()`, and
        // `has_calculated_forwarding_addresses()` was asserted above.
        ObjectReference::from_raw_address(unsafe {
            self.forwarding.forward(object.to_raw_address())
        })
        .unwrap()
    }

    /// Forward every reference slot of `object` that points into `self`.
    ///
    /// # Safety
    /// * `self.forwarding.has_calculated_forwarding_addresses()` must hold, `OVCSpace::prepare`
    ///   must not be running concurrently, and every `ObjectReference` loaded from `object`'s slots
    ///   that satisfies `self.in_space(o)` must be word-aligned inside a mapped, calculated
    ///   `[region.start(), pre_compact_cursor)` range of `self`.
    unsafe fn update_references(&self, worker: &mut GCWorker<VM>, object: ObjectReference) {
        if VM::VMScanning::support_slot_enqueuing(worker.tls, object) {
            VM::VMScanning::scan_object(worker.tls, object, &mut |s: VM::VMSlot| {
                if let Some(o) = s.load() {
                    // SAFETY: Caller guarantees forwarding readiness, exclusion of `prepare()`, and
                    // that any in-space slot target `o` resides in an allocated `OVCRegion`.
                    s.store(unsafe { self.forward(o, false) });
                }
            });
        } else {
            VM::VMScanning::scan_object_and_trace_edges(worker.tls, object, &mut |o| {
                // SAFETY: Same preconditions upheld by `update_references`'s caller.
                unsafe { self.forward(o, false) }
            });
        }
    }

    pub(crate) fn add_compact_tasks(&'static self) {
        let compact_packets: Vec<Box<dyn GCWork<VM>>> = self.generate_tasks(&mut |_, i| {
            // SAFETY: Each `i` is a unique allocated region index from `self.pr.enumerate_regions`,
            // and `Compact` is queued exclusively into STW `WorkBucketStage::Compact` after
            // `CalculateForwarding` and `SecondRoots`.
            Box::new(unsafe { Compact::<VM>::new(self, i) })
        });
        self.scheduler.work_buckets[WorkBucketStage::Compact].bulk_add(compact_packets);
    }

    /// # Safety
    /// * `index` must identify a uniquely assigned allocated region in `self.pr` compacted during
    ///   STW `WorkBucketStage::Compact` after `CalculateForwarding`, and every in-space reference
    ///   slot inside live objects of that region must target an allocated `OVCRegion` of `self.pr`.
    unsafe fn compact_region(&self, worker: &mut GCWorker<VM>, index: usize) {
        let _prepare_read = self
            .prepare_rwlock
            .try_read()
            .expect("Cannot compact OVCSpace region during prepare");
        assert!(
            self.forwarding.has_calculated_forwarding_addresses(),
            "OVCSpace::compact_region called before offset vector calculation"
        );
        self.pr.with_regions(&mut |regions| {
            let r = &regions[index];
            let start = r.region.start();
            let end = r.cursor();
            #[cfg(feature = "vo_bit")]
            {
                #[cfg(debug_assertions)]
                // SAFETY: `[start, end)` is page-aligned (`>= Block::BYTES`) within allocated
                // `OVCRegion` `r.region` with mapped, initialized `MARK_SPEC`, and `_prepare_read`
                // excludes concurrent non-atomic `prepare()`.
                unsafe {
                    self.forwarding.scan_marked_objects(
                        start,
                        end,
                        &mut |object: ObjectReference| {
                            debug_assert!(
                                crate::util::metadata::vo_bit::is_vo_bit_set(object),
                                "{:x}: VO bit not set",
                                object
                            );
                        },
                    );
                }
                crate::util::metadata::vo_bit::bzero_vo_bit(start, end - start);
            }
            let mut to = start;
            let mut compact_obj = |obj: ObjectReference| {
                // We set the end bits based on the sizes of objects when they are
                // marked, and we compute the live data and thus the forwarding
                // addresses based on those sizes. The forwarding addresses would be
                // incorrect if the sizes of objects were to change.
                let copied_size = VM::VMObjectModel::get_size_when_copied(obj);
                debug_assert!(copied_size == VM::VMObjectModel::get_current_size(obj));
                // SAFETY: `obj` was yielded from mapped, calculated `[start, end)` of `r.region`,
                // `has_calculated_forwarding_addresses()` was asserted above, and `_prepare_read`
                // excludes `prepare()`.
                let new_object = unsafe { self.forward(obj, false) };
                debug_assert!(
                    new_object.to_raw_address() >= to,
                    "whilst forwarding {obj}, the new address {0} should be after the end of the last object {to}",
                    new_object.to_raw_address()
                );
                // copy object
                trace!(" copy from {} to {}", obj, new_object);
                let end_of_new_object =
                    VM::VMObjectModel::copy_to(obj, new_object, Address::ZERO);
                // update VO bit
                #[cfg(feature = "vo_bit")]
                vo_bit::set_vo_bit(new_object);
                to = new_object.to_object_start::<VM>() + copied_size;
                debug_assert_eq!(end_of_new_object, to);
                // SAFETY: `compact_region` caller guarantees live object slots targeting `self`
                // point into mapped, calculated `[region.start(), pre_compact_cursor)` ranges of
                // `self`, under `_prepare_read` with `has_calculated_forwarding_addresses()` verified.
                unsafe { self.update_references(worker, new_object) };
            };
            // SAFETY: `[start, end)` is `Block`-aligned within allocated `OVCRegion` `r.region`
            // uniquely compacted by this packet under `_prepare_read`.
            unsafe {
                self.forwarding
                    .scan_marked_objects(start, end, &mut compact_obj);
            }
            self.pr.reset_cursor(r, to);
        });
    }

    pub(crate) fn after_compact(&self, worker: &mut GCWorker<VM>, los: &LargeObjectSpace<VM>) {
        self.pr.reset_allocator();
        let _prepare_read = self
            .prepare_rwlock
            .try_read()
            .expect("Cannot run OVCSpace::after_compact during prepare");
        // Borrow `calculated_regions` once across the cold LOS enumeration so every LOS -> OVC slot
        // is verified against `[region.start(), pre_compact_cursor)` (preserving pre-compaction
        // ranges after `compact_region` called `reset_cursor`) without per-slot locking.
        let regions = self
            .forwarding
            .calculated_regions_if_ready()
            .expect("OVCSpace::after_compact called before offset vector calculation");
        let forward_checked = |o: ObjectReference| -> ObjectReference {
            if !self.in_space(o) {
                return o;
            }
            Self::assert_in_calculated_regions(&regions, true, o);
            // SAFETY: `regions` pins `calculated == true`, `assert_in_calculated_regions` verified
            // `o` is word-aligned in a mapped, calculated `[region.start(), pre_compact_cursor)`
            // range of `self`, and `_prepare_read` excludes concurrent non-atomic `prepare()`.
            unsafe { self.forward(o, false) }
        };
        // Update references from the LOS to OVC too.
        // SAFETY: `LargeObjectSpace::enumerate_to_space_objects` enumerates treadmill cells via
        // `visit_object` and never invokes `visit_address_range`.
        let mut enumerator = unsafe {
            object_enum::ClosureObjectEnumerator::<_, VM>::new(|object: ObjectReference| {
                if VM::VMScanning::support_slot_enqueuing(worker.tls, object) {
                    VM::VMScanning::scan_object(worker.tls, object, &mut |s: VM::VMSlot| {
                        if let Some(o) = s.load() {
                            s.store(forward_checked(o));
                        }
                    });
                } else {
                    VM::VMScanning::scan_object_and_trace_edges(worker.tls, object, &mut |o| {
                        forward_checked(o)
                    });
                }
            })
        };
        los.enumerate_to_space_objects(&mut enumerator);
    }
}

/// Calculate the offset vector for a region.
pub struct CalculateOffsetVector<VM: VMBinding> {
    ovc_space: &'static OVCSpace<VM>,
    region: forwarding::OVCRegion,
    cursor: Address,
}

impl<VM: VMBinding> GCWork<VM> for CalculateOffsetVector<VM> {
    fn do_work(&mut self, _worker: &mut GCWorker<VM>, _mmtk: &'static MMTK<VM>) {
        // SAFETY: `CalculateOffsetVector` is constructed solely in `OVCSpace::add_offset_vector_tasks`
        // for an allocated `OVCSpace` region and executed during STW `CalculateForwarding` while
        // `MARK_SPEC` is quiescent.
        unsafe {
            self.ovc_space
                .calculate_offset_vector_for_region(self.region, self.cursor);
        }
    }
}

impl<VM: VMBinding> CalculateOffsetVector<VM> {
    /// # Safety
    /// `region` and `cursor` must satisfy [`forwarding::ForwardingMetadata::calculate_offset_vector`]
    /// when `do_work` executes in `WorkBucketStage::CalculateForwarding`.
    unsafe fn new(
        ovc_space: &'static OVCSpace<VM>,
        region: forwarding::OVCRegion,
        cursor: Address,
    ) -> Self {
        Self {
            ovc_space,
            region,
            cursor,
        }
    }
}

/// Compact live objects in a region.
pub struct Compact<VM: VMBinding> {
    ovc_space: &'static OVCSpace<VM>,
    index: usize,
}

impl<VM: VMBinding> GCWork<VM> for Compact<VM> {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, _mmtk: &'static MMTK<VM>) {
        // SAFETY: `Compact` is constructed solely in `OVCSpace::add_compact_tasks` with a unique
        // region index and executed during STW `WorkBucketStage::Compact` while `MARK_SPEC` is quiescent.
        unsafe {
            self.ovc_space.compact_region(worker, self.index);
        }
    }
}

impl<VM: VMBinding> Compact<VM> {
    /// # Safety
    /// `index` must be a uniquely assigned region in `ovc_space` compacted during STW `Compact`.
    unsafe fn new(ovc_space: &'static OVCSpace<VM>, index: usize) -> Self {
        Self { ovc_space, index }
    }
}
