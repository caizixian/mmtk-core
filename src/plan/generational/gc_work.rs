use atomic::Ordering;

use crate::plan::PlanTraceObject;
use crate::plan::VectorObjectQueue;
use crate::policy::gc_work::TraceKind;
use crate::scheduler::{gc_work::*, GCWork, GCWorker, WorkBucketStage};
use crate::util::ObjectReference;
use crate::vm::slot::{MemorySlice, Slot};
use crate::vm::*;
use crate::MMTK;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use super::global::GenerationalPlanExt;

/// Size of the forwarding pointer cache in GenNurseryProcessEdges.
/// 256 entries × 16 bytes = 4KB, fits comfortably in L1 data cache.
const FWD_CACHE_SIZE: usize = 256;

/// Process edges for a nursery GC. This type is provided if a generational plan does not use
/// [`crate::scheduler::gc_work::SFTProcessEdges`]. If a plan uses `SFTProcessEdges`,
/// it does not need to use this type.
pub struct GenNurseryProcessEdges<
    VM: VMBinding,
    P: GenerationalPlanExt<VM> + PlanTraceObject<VM>,
    const KIND: TraceKind,
> {
    plan: &'static P,
    base: ProcessEdgesBase<VM>,
    /// A direct-mapped cache mapping original nursery object addresses to their forwarded
    /// addresses. When multiple slots in a work packet reference the same nursery object,
    /// the cache avoids redundant trace_object calls (which include in_space checks and
    /// CAS on the forwarding word). 256 entries × 16 bytes = 4KB, fits in L1 data cache.
    fwd_cache: [(usize, usize); FWD_CACHE_SIZE],
}

impl<VM: VMBinding, P: GenerationalPlanExt<VM> + PlanTraceObject<VM>, const KIND: TraceKind>
    GenNurseryProcessEdges<VM, P, KIND>
{
    /// Compute a cache index from an object address.
    /// Objects are at least 8-byte aligned, so shift right by 3 to get a better hash.
    #[inline(always)]
    fn cache_index(addr: usize) -> usize {
        // Mix bits to spread adjacent addresses across cache entries.
        // Shift by 3 (8-byte alignment) and XOR with upper bits for better distribution.
        ((addr >> 3) ^ (addr >> 11)) & (FWD_CACHE_SIZE - 1)
    }
}

impl<VM: VMBinding, P: GenerationalPlanExt<VM> + PlanTraceObject<VM>, const KIND: TraceKind>
    ProcessEdgesWork for GenNurseryProcessEdges<VM, P, KIND>
{
    type VM = VM;
    type ScanObjectsWorkType = PlanScanObjects<Self, P>;

    fn new(
        slots: Vec<SlotOf<Self>>,
        roots: bool,
        mmtk: &'static MMTK<VM>,
        bucket: WorkBucketStage,
    ) -> Self {
        let base = ProcessEdgesBase::new(slots, roots, mmtk, bucket);
        let plan = base.plan().downcast_ref().unwrap();
        Self {
            plan,
            base,
            fwd_cache: [(0, 0); FWD_CACHE_SIZE],
        }
    }

    fn trace_object(&mut self, object: ObjectReference) -> ObjectReference {
        // We cannot borrow `self` twice in a call, so we extract `worker` as a local variable.
        let worker = self.worker();
        self.plan.trace_object_nursery::<VectorObjectQueue, KIND>(
            &mut self.base.nodes,
            object,
            worker,
        )
    }

    fn process_slot(&mut self, slot: SlotOf<Self>) {
        let Some(object) = slot.load() else {
            // Skip slots that are not holding an object reference.
            return;
        };

        let addr = object.to_raw_address().as_usize();
        let idx = Self::cache_index(addr);

        // Check forwarding cache: if this object was already forwarded in this work packet,
        // reuse the cached result to avoid redundant trace_object (in_space check + CAS).
        let (cached_orig, cached_fwd) = self.fwd_cache[idx];
        if cached_orig == addr && cached_orig != cached_fwd && cached_orig != 0 {
            // Cache hit: object was forwarded earlier in this work packet.
            // Safety: cached_fwd was produced by trace_object, so it's a valid ObjectReference.
            let new_object =
                unsafe { ObjectReference::from_raw_address_unchecked(crate::util::Address::from_usize(cached_fwd)) };
            slot.store(new_object);
            return;
        }

        let new_object = self.trace_object(object);
        debug_assert!(!self.plan.is_object_in_nursery(new_object));

        // Update cache with the mapping (original -> forwarded).
        // For mature objects (new_object == object), we don't cache since there's no benefit:
        // the trace_object for mature objects is already cheap (just an in_space check + return).
        if new_object != object {
            self.fwd_cache[idx] = (addr, new_object.to_raw_address().as_usize());
            slot.store(new_object);
        }
    }

    fn create_scan_work(&self, nodes: Vec<ObjectReference>) -> Self::ScanObjectsWorkType {
        PlanScanObjects::new(self.plan, nodes, false, self.bucket)
    }
}

impl<VM: VMBinding, P: GenerationalPlanExt<VM> + PlanTraceObject<VM>, const KIND: TraceKind> Deref
    for GenNurseryProcessEdges<VM, P, KIND>
{
    type Target = ProcessEdgesBase<VM>;
    fn deref(&self) -> &Self::Target {
        &self.base
    }
}

impl<VM: VMBinding, P: GenerationalPlanExt<VM> + PlanTraceObject<VM>, const KIND: TraceKind>
    DerefMut for GenNurseryProcessEdges<VM, P, KIND>
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.base
    }
}

/// The modbuf contains a list of objects in mature space(s) that
/// may contain pointers to the nursery space.
/// This work packet scans the recorded objects and forwards pointers if necessary.
pub struct ProcessModBuf<E: ProcessEdgesWork> {
    modbuf: Vec<ObjectReference>,
    phantom: PhantomData<E>,
}

impl<E: ProcessEdgesWork> ProcessModBuf<E> {
    pub fn new(modbuf: Vec<ObjectReference>) -> Self {
        debug_assert!(!modbuf.is_empty());
        Self {
            modbuf,
            phantom: PhantomData,
        }
    }
}

impl<E: ProcessEdgesWork> GCWork<E::VM> for ProcessModBuf<E> {
    fn do_work(&mut self, worker: &mut GCWorker<E::VM>, mmtk: &'static MMTK<E::VM>) {
        // Process and scan modbuf only if the current GC is a nursery GC
        let gen = mmtk.get_plan().generational().unwrap();
        if gen.is_current_gc_nursery() {
            // Flip the per-object unlogged bits to "unlogged" state.
            for obj in &self.modbuf {
                debug_assert!(
                    !gen.is_object_in_nursery(*obj),
                    "{} was logged but is not mature. Dumping process memory maps:\n{}",
                    *obj,
                    crate::util::memory::get_process_memory_maps(),
                );
                <E::VM as VMBinding>::VMObjectModel::GLOBAL_LOG_BIT_SPEC.store_atomic::<E::VM, u8>(
                    *obj,
                    1,
                    None,
                    Ordering::SeqCst,
                );
            }
            // Scan objects in the modbuf and forward pointers
            let modbuf = std::mem::take(&mut self.modbuf);
            GCWork::do_work(
                &mut ScanObjects::<E>::new(modbuf, false, WorkBucketStage::Closure),
                worker,
                mmtk,
            )
        }
    }
}

/// The array-copy modbuf contains a list of array slices in mature space(s) that
/// may contain pointers to the nursery space.
/// This work packet forwards and updates each entry in the recorded slices.
pub struct ProcessRegionModBuf<E: ProcessEdgesWork> {
    /// A list of `(start_address, bytes)` tuple.
    modbuf: Vec<<E::VM as VMBinding>::VMMemorySlice>,
    phantom: PhantomData<E>,
}

impl<E: ProcessEdgesWork> ProcessRegionModBuf<E> {
    pub fn new(modbuf: Vec<<E::VM as VMBinding>::VMMemorySlice>) -> Self {
        Self {
            modbuf,
            phantom: PhantomData,
        }
    }
}

impl<E: ProcessEdgesWork> GCWork<E::VM> for ProcessRegionModBuf<E> {
    fn do_work(&mut self, worker: &mut GCWorker<E::VM>, mmtk: &'static MMTK<E::VM>) {
        // Scan modbuf only if the current GC is a nursery GC
        if mmtk
            .get_plan()
            .generational()
            .unwrap()
            .is_current_gc_nursery()
        {
            // Collect all the entries in all the slices
            let mut slots = vec![];
            for slice in &self.modbuf {
                for slot in slice.iter_slots() {
                    slots.push(slot);
                }
            }
            // Forward entries
            GCWork::do_work(
                &mut E::new(slots, false, mmtk, WorkBucketStage::Closure),
                worker,
                mmtk,
            )
        }
    }
}
