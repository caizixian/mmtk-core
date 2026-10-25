#![deny(unsafe_op_in_unsafe_fn)]

use crate::plan::tracing::OptionObjectQueue;
use crate::policy::gc_work::{TraceKind, TRACE_KIND_TRANSITIVE_PIN};
use crate::policy::largeobjectspace::LargeObjectSpace;
use crate::policy::ovc::forwarding::{
    self, CalculatedRegionEntry, ForwardingMetadata, MappedOVCRegion,
};
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
use crate::util::object_enum::ObjectEnumerator;
use crate::util::{Address, ObjectReference};
use crate::vm::slot::Slot;
use crate::MMTK;
use crate::{vm::*, ObjectQueue};
use atomic::Ordering;
use std::sync::Arc;

pub(crate) const TRACE_KIND_MARK: TraceKind = 0;
pub(crate) const TRACE_KIND_FORWARD_ROOT: TraceKind = 1;

struct LosObjectVisitor<F>(F);

impl<F: FnMut(ObjectReference)> ObjectEnumerator for LosObjectVisitor<F> {
    fn visit_object(&mut self, object: ObjectReference) {
        (self.0)(object);
    }

    unsafe fn visit_address_range(&mut self, _start: Address, _end: Address) {
        unreachable!("LargeObjectSpace::enumerate_to_space_objects never visits address ranges")
    }
}

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
    forwarding: ForwardingMetadata<VM>,
    scheduler: Arc<GCWorkScheduler<VM>>,
}

impl<VM: VMBinding> SFT for OVCSpace<VM> {
    fn name(&self) -> &'static str {
        self.get_name()
    }

    fn get_forwarded_object(&self, object: ObjectReference) -> Option<ObjectReference> {
        let regions = self.forwarding.calculated_regions_if_ready()?;
        assert!(
            self.in_space(object),
            "Object {object} does not belong to OVCSpace"
        );
        Some(ForwardingMetadata::<VM>::forward_in_regions(
            &regions, object,
        ))
    }

    fn is_live(&self, object: ObjectReference) -> bool {
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
        self.assert_mark_mapped_in_space(object);
        println!("marked = {}", OVCSpace::<VM>::is_marked(object));
        let forwarded = self
            .forwarding
            .calculated_regions_if_ready()
            .map(|regions| ForwardingMetadata::<VM>::forward_in_regions(&regions, object));
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
            forwarding: ForwardingMetadata::new(),
            common,
            scheduler,
        }
    }

    fn assert_mark_mapped_in_space(&self, object: ObjectReference) {
        let addr = object.to_raw_address();
        let valid = self.in_space(object)
            && addr.is_aligned_to(ObjectReference::ALIGNMENT)
            && addr.is_aligned_to(BYTES_IN_WORD)
            && forwarding::MARK_SPEC.is_mapped(addr)
            && (self
                .forwarding
                .calculated_regions_if_ready()
                .is_some_and(|regions| {
                    ForwardingMetadata::<VM>::find_region(&regions, addr).is_some()
                })
                || self.pr.with_regions(&mut |regions| {
                    let target_start = addr.align_down(forwarding::OVCRegion::BYTES);
                    regions
                        .iter()
                        .any(|r| r.region.start() == target_start && addr < r.cursor())
                }));
        assert!(
            valid,
            "Object {object} is not in a mapped allocated OVCSpace region"
        );
    }

    pub(crate) fn prepare(&self) {
        self.forwarding.reset();
        self.pr
            .enumerate_regions(&mut |r: &AllocatedRegion<forwarding::OVCRegion>| {
                MappedOVCRegion::from_allocated_prefix(r.region, r.cursor()).clear_mark_bits();
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
        #[cfg(feature = "vo_bit")]
        debug_assert!(
            crate::util::metadata::vo_bit::is_vo_bit_set(object),
            "{:x}: VO bit not set",
            object
        );
        let regions = self
            .forwarding
            .calculated_regions_if_ready()
            .expect("OVCSpace::trace_forward_root called before offset vector calculation");
        ForwardingMetadata::<VM>::forward_in_regions(&regions, object)
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

    pub(crate) fn add_offset_vector_tasks(&'static self) {
        let mut calculated_regions = Vec::new();
        let mut offset_vector_packets: Vec<Box<dyn GCWork<VM>>> = Vec::new();
        let mut alloc_index = 0usize;
        self.pr.enumerate_regions(&mut |r| {
            let mapped = MappedOVCRegion::from_allocated_prefix(r.region, r.cursor());
            calculated_regions.push(CalculatedRegionEntry {
                alloc_index,
                mapped,
            });
            offset_vector_packets.push(Box::new(CalculateOffsetVector::<VM>::new(self, mapped)));
            alloc_index += 1;
        });
        self.forwarding.begin_calculate(calculated_regions);
        self.scheduler.work_buckets[WorkBucketStage::CalculateForwarding]
            .bulk_add(offset_vector_packets);
    }

    fn calculate_offset_vector_for_region(&self, region: MappedOVCRegion) {
        region.calculate_offset_vector();
        self.forwarding.finish_region_calculation();
    }

    #[inline]
    fn forward_slot_target(
        &self,
        local_region: &MappedOVCRegion,
        all_regions: &[CalculatedRegionEntry],
        object: ObjectReference,
    ) -> ObjectReference {
        if !self.in_space(object) {
            return object;
        }
        let addr = object.to_raw_address();
        if local_region.contains(addr) {
            assert!(
                addr.is_aligned_to(ObjectReference::ALIGNMENT) && addr.is_aligned_to(BYTES_IN_WORD),
                "Object {object} is not word/ObjectReference-aligned in OVCSpace"
            );
            return ObjectReference::from_raw_address(local_region.forward(addr)).unwrap();
        }
        ForwardingMetadata::<VM>::forward_in_regions(all_regions, object)
    }

    fn update_references(
        &self,
        worker: &mut GCWorker<VM>,
        local_region: &MappedOVCRegion,
        all_regions: &[CalculatedRegionEntry],
        object: ObjectReference,
    ) {
        if VM::VMScanning::support_slot_enqueuing(worker.tls, object) {
            VM::VMScanning::scan_object(worker.tls, object, &mut |s: VM::VMSlot| {
                if let Some(o) = s.load() {
                    s.store(self.forward_slot_target(local_region, all_regions, o));
                }
            });
        } else {
            VM::VMScanning::scan_object_and_trace_edges(worker.tls, object, &mut |o| {
                self.forward_slot_target(local_region, all_regions, o)
            });
        }
    }

    pub(crate) fn add_compact_tasks(&'static self) {
        let regions = self
            .forwarding
            .calculated_regions_if_ready()
            .expect("OVCSpace::add_compact_tasks called before offset vector calculation");
        let compact_packets: Vec<Box<dyn GCWork<VM>>> = regions
            .iter()
            .map(|entry| {
                Box::new(Compact::<VM>::new(
                    self,
                    entry.alloc_index,
                    entry.mapped,
                    Arc::clone(&regions),
                )) as Box<dyn GCWork<VM>>
            })
            .collect();
        self.scheduler.work_buckets[WorkBucketStage::Compact].bulk_add(compact_packets);
    }

    fn compact_region(
        &self,
        worker: &mut GCWorker<VM>,
        alloc_index: usize,
        region: MappedOVCRegion,
        all_regions: &[CalculatedRegionEntry],
    ) {
        assert!(
            self.forwarding.has_calculated_forwarding_addresses(),
            "OVCSpace::compact_region called before offset vector calculation"
        );
        let start = region.region().start();
        #[cfg(feature = "vo_bit")]
        {
            let end = region.cursor();
            #[cfg(debug_assertions)]
            region.scan_marked_objects(&mut |object: ObjectReference| {
                debug_assert!(
                    crate::util::metadata::vo_bit::is_vo_bit_set(object),
                    "{:x}: VO bit not set",
                    object
                );
            });
            crate::util::metadata::vo_bit::bzero_vo_bit(start, end - start);
        }
        let mut to = start;
        region.scan_marked_objects(&mut |obj: ObjectReference| {
            // We set the end bits based on the sizes of objects when they are
            // marked, and we compute the live data and thus the forwarding
            // addresses based on those sizes. The forwarding addresses would be
            // incorrect if the sizes of objects were to change.
            let copied_size = VM::VMObjectModel::get_size_when_copied(obj);
            debug_assert!(copied_size == VM::VMObjectModel::get_current_size(obj));
            let new_object =
                ObjectReference::from_raw_address(region.forward(obj.to_raw_address())).unwrap();
            debug_assert!(
                new_object.to_raw_address() >= to,
                "whilst forwarding {obj}, the new address {0} should be after the end of the last object {to}",
                new_object.to_raw_address()
            );
            // copy object
            trace!(" copy from {} to {}", obj, new_object);
            let end_of_new_object = VM::VMObjectModel::copy_to(obj, new_object, Address::ZERO);
            // update VO bit
            #[cfg(feature = "vo_bit")]
            vo_bit::set_vo_bit(new_object);
            to = new_object.to_object_start::<VM>() + copied_size;
            debug_assert_eq!(end_of_new_object, to);
            self.update_references(worker, &region, all_regions, new_object);
        });
        self.pr.with_regions(&mut |regions| {
            self.pr.reset_cursor(&regions[alloc_index], to);
        });
    }

    pub(crate) fn after_compact(&self, worker: &mut GCWorker<VM>, los: &LargeObjectSpace<VM>) {
        self.pr.reset_allocator();
        let regions = self
            .forwarding
            .calculated_regions_if_ready()
            .expect("OVCSpace::after_compact called before offset vector calculation");
        let forward_checked = |o: ObjectReference| -> ObjectReference {
            if !self.in_space(o) {
                return o;
            }
            ForwardingMetadata::<VM>::forward_in_regions(&regions, o)
        };
        // Update references from the LOS to OVC too.
        let mut visitor = LosObjectVisitor(|object: ObjectReference| {
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
        });
        los.enumerate_to_space_objects(&mut visitor);
    }
}

/// Calculate the offset vector for a region.
pub struct CalculateOffsetVector<VM: VMBinding> {
    ovc_space: &'static OVCSpace<VM>,
    region: MappedOVCRegion,
}

impl<VM: VMBinding> GCWork<VM> for CalculateOffsetVector<VM> {
    fn do_work(&mut self, _worker: &mut GCWorker<VM>, _mmtk: &'static MMTK<VM>) {
        self.ovc_space
            .calculate_offset_vector_for_region(self.region);
    }
}

impl<VM: VMBinding> CalculateOffsetVector<VM> {
    fn new(ovc_space: &'static OVCSpace<VM>, region: MappedOVCRegion) -> Self {
        Self { ovc_space, region }
    }
}

/// Compact live objects in a region.
pub struct Compact<VM: VMBinding> {
    ovc_space: &'static OVCSpace<VM>,
    alloc_index: usize,
    region: MappedOVCRegion,
    all_regions: Arc<[CalculatedRegionEntry]>,
}

impl<VM: VMBinding> GCWork<VM> for Compact<VM> {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, _mmtk: &'static MMTK<VM>) {
        self.ovc_space
            .compact_region(worker, self.alloc_index, self.region, &self.all_regions);
    }
}

impl<VM: VMBinding> Compact<VM> {
    fn new(
        ovc_space: &'static OVCSpace<VM>,
        alloc_index: usize,
        region: MappedOVCRegion,
        all_regions: Arc<[CalculatedRegionEntry]>,
    ) -> Self {
        Self {
            ovc_space,
            alloc_index,
            region,
            all_regions,
        }
    }
}
