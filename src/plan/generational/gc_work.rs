use atomic::Ordering;

use crate::plan::TracePolicy;
use crate::scheduler::gc_work::{GCProcessEdges, GCScanObjects};
use crate::scheduler::{GCWork, GCWorker, WorkBucketStage};
use crate::util::ObjectReference;
use crate::vm::slot::MemorySlice;
use crate::vm::*;
use crate::MMTK;
use std::marker::PhantomData;

/// The modbuf contains a list of objects in mature space(s) that
/// may contain pointers to the nursery space.
/// This work packet scans the recorded objects and forwards pointers if necessary.
pub struct ProcessModBuf<VM: VMBinding, T: TracePolicy<VM>> {
    modbuf: Vec<ObjectReference>,
    phantom: PhantomData<(VM, T)>,
}

impl<VM: VMBinding, T: TracePolicy<VM>> ProcessModBuf<VM, T> {
    pub fn new(modbuf: Vec<ObjectReference>) -> Self {
        debug_assert!(!modbuf.is_empty());
        Self {
            modbuf,
            phantom: PhantomData,
        }
    }
}

impl<VM: VMBinding, T: TracePolicy<VM>> GCWork<VM> for ProcessModBuf<VM, T> {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, mmtk: &'static MMTK<VM>) {
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
                VM::VMObjectModel::GLOBAL_LOG_BIT_SPEC.store_atomic::<VM, u8>(
                    *obj,
                    1,
                    None,
                    Ordering::SeqCst,
                );
            }
            // Scan objects in the modbuf and forward pointers
            let modbuf = std::mem::take(&mut self.modbuf);
            GCWork::do_work(
                &mut GCScanObjects::<VM, T>::new(
                    T::from_mmtk(mmtk),
                    modbuf,
                    false,
                    WorkBucketStage::Closure,
                ),
                worker,
                mmtk,
            )
        }
    }
}

/// The array-copy modbuf contains a list of array slices in mature space(s) that
/// may contain pointers to the nursery space.
/// This work packet forwards and updates each entry in the recorded slices.
pub struct ProcessRegionModBuf<VM: VMBinding, T: TracePolicy<VM>> {
    /// A list of `(start_address, bytes)` tuple.
    modbuf: Vec<VM::VMMemorySlice>,
    phantom: PhantomData<(VM, T)>,
}

impl<VM: VMBinding, T: TracePolicy<VM>> ProcessRegionModBuf<VM, T> {
    pub fn new(modbuf: Vec<VM::VMMemorySlice>) -> Self {
        Self {
            modbuf,
            phantom: PhantomData,
        }
    }
}

impl<VM: VMBinding, T: TracePolicy<VM>> GCWork<VM> for ProcessRegionModBuf<VM, T> {
    fn do_work(&mut self, worker: &mut GCWorker<VM>, mmtk: &'static MMTK<VM>) {
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
                &mut GCProcessEdges::<VM, T>::new(slots, false, mmtk, WorkBucketStage::Closure),
                worker,
                mmtk,
            )
        }
    }
}
