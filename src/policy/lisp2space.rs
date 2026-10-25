use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{RwLock, RwLockWriteGuard};

use super::sft::SFT;
use super::space::{CommonSpace, Space};
use crate::plan::tracing::{ObjectQueue, OptionObjectQueue};
use crate::policy::gc_work::{TraceKind, TRACE_KIND_TRANSITIVE_PIN};
use crate::policy::sft::GCWorkerMutRef;
use crate::scheduler::GCWorker;
use crate::util::alloc::allocator::align_allocation_no_fill;
use crate::util::constants::LOG_BYTES_IN_WORD;
use crate::util::copy::CopySemantics;
use crate::util::heap::{MonotonePageResource, PageResource};
use crate::util::metadata::{extract_side_metadata, vo_bit};
use crate::util::object_enum::{self, ObjectEnumerator};
use crate::util::{Address, ObjectReference};
use crate::vm::*;
use atomic::Ordering;

pub(crate) const TRACE_KIND_MARK: TraceKind = 0;
pub(crate) const TRACE_KIND_FORWARD: TraceKind = 1;

pub struct Lisp2Space<VM: VMBinding> {
    common: CommonSpace<VM>,
    pr: MonotonePageResource<VM>,
    /// Enforces mutual exclusion between single-threaded `calculate_forwarding_pointer` and
    /// `compact` passes even if invoked through safe `&self` references.
    pass_active: AtomicBool,
    /// Set while `compact` is executing non-atomic `VMObjectModel::copy_to` slides so concurrent
    /// forwarding-pointer readers fail fast via `assert!` instead of data-racing with `copy_to`.
    in_compact: AtomicBool,
    /// Synchronizes cold external SFT readers (`get_forwarded_object`, `debug_print_object_info`)
    /// against `compact` so an in-flight reader cannot overlap `copy_to` across preemption.
    compact_rwlock: RwLock<()>,
}

struct Lisp2PassGuard<'a, VM: VMBinding> {
    space: &'a Lisp2Space<VM>,
    _compact_write: Option<RwLockWriteGuard<'a, ()>>,
}

impl<'a, VM: VMBinding> Lisp2PassGuard<'a, VM> {
    fn acquire(space: &'a Lisp2Space<VM>, is_compact: bool) -> Self {
        assert!(
            !space.pass_active.swap(true, Ordering::AcqRel),
            "Concurrent Lisp2Space forwarding/compaction pass detected"
        );
        let compact_write = if is_compact {
            let guard = match space.compact_rwlock.try_write() {
                Ok(guard) => guard,
                Err(_) => {
                    space.pass_active.store(false, Ordering::Release);
                    panic!("Cannot run Lisp2Space::compact concurrently with forwarding-pointer inspection");
                }
            };
            space.in_compact.store(true, Ordering::Release);
            Some(guard)
        } else {
            None
        };
        Self {
            space,
            _compact_write: compact_write,
        }
    }
}

impl<VM: VMBinding> Drop for Lisp2PassGuard<'_, VM> {
    fn drop(&mut self) {
        if self._compact_write.is_some() {
            self.space.in_compact.store(false, Ordering::Release);
        }
        self.space.pass_active.store(false, Ordering::Release);
    }
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
        let _compact_read = self
            .compact_rwlock
            .try_read()
            .expect("Cannot inspect Lisp2Space forwarding pointer during compact");
        self.assert_valid_object_in_space(object);
        // SAFETY: `assert_valid_object_in_space` verified `object` is an allocated `Lisp2Space`
        // object in `self` whose `obj_start - GC_EXTRA_HEADER_BYTES` word is mapped inside `self`
        // and aligned for `AtomicUsize`, `_compact_read` prevents concurrent `compact` (`copy_to`)
        // execution for the duration of this read, and all forwarding-word accesses use
        // `AtomicUsize` (`Ordering::Relaxed`).
        unsafe { Self::get_header_forwarding_pointer(object) }
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
        let _compact_read = self
            .compact_rwlock
            .try_read()
            .expect("Cannot inspect Lisp2Space forwarding pointer during compact");
        self.assert_valid_object_in_space(object);
        println!("marked = {}", Lisp2Space::<VM>::is_marked(object));
        println!(
            "head forwarding pointer = {:?}",
            // SAFETY: `assert_valid_object_in_space` verified `object` is an allocated `Lisp2Space`
            // object in `self` with a mapped, initialized, `AtomicUsize`-aligned forwarding header
            // word, `_compact_read` prevents concurrent `compact` (`copy_to`) execution, and all
            // forwarding-word accesses use `AtomicUsize`.
            unsafe { Lisp2Space::<VM>::get_header_forwarding_pointer(object) }
        );
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

    fn initialize_sft(&self, sft_map: &mut dyn crate::policy::sft_map::SFTMap) {
        self.common().initialize_sft(self.as_sft(), sft_map)
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
    fn trace_object<Q: ObjectQueue, const KIND: crate::policy::gc_work::TraceKind>(
        &self,
        queue: &mut Q,
        object: ObjectReference,
        _copy: Option<CopySemantics>,
        _worker: &mut GCWorker<VM>,
    ) -> ObjectReference {
        debug_assert!(
            KIND != TRACE_KIND_TRANSITIVE_PIN,
            "Lisp2 does not support transitive pin trace."
        );
        if KIND == TRACE_KIND_MARK {
            self.trace_mark_object(queue, object)
        } else if KIND == TRACE_KIND_FORWARD {
            self.trace_forward_object(queue, object)
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

    // The following are a few functions for manipulating header forwarding poiner.
    // Basically for each allocation request, we allocate extra bytes of [`HEADER_RESERVED_IN_BYTES`].
    // From the allocation result we get (e.g. `alloc_res`), `alloc_res + HEADER_RESERVED_IN_BYTES` is the cell
    // address we return to the binding. It ensures we have at least one word (`GC_EXTRA_HEADER_WORD`) before
    // the cell address, and ensures the cell address is properly aligned.
    // From the cell address, `cell - GC_EXTRA_HEADER_WORD` is where we store the header forwarding pointer.

    fn assert_valid_object_in_space(&self, object: ObjectReference) {
        assert!(
            !self.in_compact.load(Ordering::Acquire),
            "Cannot access Lisp2Space forwarding pointer during compaction"
        );
        let raw = object.to_raw_address();
        let obj_start = object.to_object_start::<VM>();
        assert!(
            self.in_space(object)
                && raw.is_aligned_to(ObjectReference::ALIGNMENT)
                && vo_bit::VO_BIT_SIDE_METADATA_SPEC.is_mapped(raw)
                && vo_bit::VO_BIT_SIDE_METADATA_SPEC.load_atomic::<u8>(raw, Ordering::Relaxed) == 1
                && obj_start.as_usize() >= GC_EXTRA_HEADER_BYTES
                && self.address_in_space(obj_start - GC_EXTRA_HEADER_BYTES)
                && (obj_start - GC_EXTRA_HEADER_BYTES)
                    .is_aligned_to(std::mem::align_of::<AtomicUsize>()),
            "Object {object} is not a valid allocated object in Lisp2Space",
        );
    }

    /// Get the address for header forwarding pointer
    fn header_forwarding_pointer_address(object: ObjectReference) -> Address {
        object.to_object_start::<VM>() - GC_EXTRA_HEADER_BYTES
    }

    /// Get header forwarding pointer for an object in `Lisp2Space`.
    ///
    /// # Safety
    /// - `object` must reside in mapped `Lisp2Space` memory allocated with `HEADER_RESERVED_IN_BYTES`
    ///   before `object.to_object_start::<VM>()`, so `object.to_object_start::<VM>() - GC_EXTRA_HEADER_BYTES`
    ///   is mapped, initialized, and aligned to `align_of::<AtomicUsize>()`.
    /// - No thread may concurrently perform non-atomic writes (such as `compact`'s
    ///   `VMObjectModel::copy_to`) to `object`'s header forwarding pointer word.
    unsafe fn get_header_forwarding_pointer(object: ObjectReference) -> Option<ObjectReference> {
        let fwd_addr = Self::header_forwarding_pointer_address(object);
        debug_assert!(fwd_addr.is_aligned_to(std::mem::align_of::<AtomicUsize>()));
        // SAFETY: Caller guarantees `fwd_addr` is mapped, initialized, aligned for `AtomicUsize`,
        // and not subject to concurrent non-atomic accesses; all forwarding-word reads/writes use
        // `AtomicUsize`.
        let raw = unsafe { (&*fwd_addr.to_ptr::<AtomicUsize>()).load(Ordering::Relaxed) };
        // SAFETY: `Address::from_usize` wraps the raw forwarding pointer word (`0` if unforwarded,
        // or a valid `ObjectReference` raw address written by `store_header_forwarding_pointer`).
        let addr = unsafe { Address::from_usize(raw) };
        ObjectReference::from_raw_address(addr)
    }

    /// Store header forwarding pointer for an object in `Lisp2Space`.
    ///
    /// # Safety
    /// - `object` must reside in mapped `Lisp2Space` memory allocated with `HEADER_RESERVED_IN_BYTES`
    ///   before `object.to_object_start::<VM>()`, so `object.to_object_start::<VM>() - GC_EXTRA_HEADER_BYTES`
    ///   is mapped, initialized, and aligned to `align_of::<AtomicUsize>()`.
    /// - No thread may concurrently perform non-atomic accesses to `object`'s header forwarding
    ///   pointer word.
    unsafe fn store_header_forwarding_pointer(
        object: ObjectReference,
        forwarding_pointer: ObjectReference,
    ) {
        let fwd_addr = Self::header_forwarding_pointer_address(object);
        debug_assert!(fwd_addr.is_aligned_to(std::mem::align_of::<AtomicUsize>()));
        // SAFETY: Caller guarantees `fwd_addr` is mapped, initialized, aligned for `AtomicUsize`,
        // and not subject to concurrent non-atomic accesses.
        unsafe {
            (&*fwd_addr.to_ptr::<AtomicUsize>()).store(
                forwarding_pointer.to_raw_address().as_usize(),
                Ordering::Relaxed,
            );
        }
    }

    /// Clear header forwarding pointer word for a destination object in `Lisp2Space`.
    ///
    /// # Safety
    /// - `object` must reside in mapped `Lisp2Space` memory with `GC_EXTRA_HEADER_BYTES` reserved
    ///   before `object.to_object_start::<VM>()`, so `object.to_object_start::<VM>() - GC_EXTRA_HEADER_BYTES`
    ///   is mapped, initialized (zero-mapped by `MMAPPER`), and aligned to `align_of::<AtomicUsize>()`.
    /// - No thread may concurrently perform non-atomic accesses to `object`'s header forwarding
    ///   pointer word.
    unsafe fn clear_header_forwarding_pointer(object: ObjectReference) {
        let fwd_addr = Self::header_forwarding_pointer_address(object);
        debug_assert!(fwd_addr.is_aligned_to(std::mem::align_of::<AtomicUsize>()));
        // SAFETY: Caller guarantees `fwd_addr` is mapped, initialized, aligned for `AtomicUsize`,
        // and not subject to concurrent non-atomic accesses.
        unsafe {
            (&*fwd_addr.to_ptr::<AtomicUsize>()).store(0, Ordering::Relaxed);
        }
    }

    pub fn new(args: crate::policy::space::PlanCreateSpaceArgs<VM>) -> Self {
        let vm_map = args.vm_map;
        let is_discontiguous = args.vmrequest.is_discontiguous();
        let local_specs = extract_side_metadata(&[*VM::VMObjectModel::LOCAL_MARK_BIT_SPEC]);
        let common = CommonSpace::new(args.into_policy_args(true, false, local_specs));
        Lisp2Space {
            pr: if is_discontiguous {
                MonotonePageResource::new_discontiguous(vm_map)
            } else {
                MonotonePageResource::new_contiguous(common.start, common.extent, vm_map)
            },
            common,
            pass_active: AtomicBool::new(false),
            in_compact: AtomicBool::new(false),
            compact_rwlock: RwLock::new(()),
        }
    }

    pub fn prepare(&self) {}

    pub fn release(&self) {}

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

    pub fn trace_forward_object<Q: ObjectQueue>(
        &self,
        queue: &mut Q,
        object: ObjectReference,
    ) -> ObjectReference {
        self.assert_valid_object_in_space(object);
        // from this stage and onwards, mark bit is no longer needed
        // therefore, it can be reused to save one extra bit in metadata
        if Lisp2Space::<VM>::test_and_clear_mark(object) {
            queue.enqueue(object);
        }

        // SAFETY: `assert_valid_object_in_space` verified `!self.in_compact` and that `object` is
        // an allocated `Lisp2Space` object in `self` with a mapped, initialized, `AtomicUsize`-aligned
        // forwarding header word written during `CalculateForwarding` (separated from `Forward` by a
        // scheduler stage barrier; `Forward` runs inside private `mod policy` never overlapping `Compact`).
        unsafe { Self::get_header_forwarding_pointer(object) }
            .unwrap_or_else(|| panic!("Object {object} does not have a forwarding pointer"))
    }

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

    pub fn is_marked(object: ObjectReference) -> bool {
        let old_value = VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.load_atomic::<VM, u8>(
            object,
            None,
            Ordering::SeqCst,
        );
        let mark_bit = old_value & GC_MARK_BIT_MASK;
        mark_bit != 0
    }

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

    pub fn calculate_forwarding_pointer(&self) {
        let _pass = Lisp2PassGuard::acquire(self, false);
        let mut to_iter = self.pr.iterate_allocated_regions();
        let Some((mut to_cursor, mut to_size)) = to_iter.next() else {
            return;
        };
        let mut to_end = to_cursor + to_size;
        for (from_start, size) in self.pr.iterate_allocated_regions() {
            let from_end = from_start + size;
            // linear scan the contiguous region
            for obj in self
                .linear_scan_objects(from_start..from_end)
                .filter(Self::to_be_compacted)
            {
                let copied_size =
                    VM::VMObjectModel::get_size_when_copied(obj) + Self::HEADER_RESERVED_IN_BYTES;
                let align = VM::VMObjectModel::get_align_when_copied(obj);
                let offset = VM::VMObjectModel::get_align_offset_when_copied(obj);
                // move to_cursor to aliged start address
                to_cursor = align_allocation_no_fill::<VM>(to_cursor, align, offset);
                // move to next to-block if there is no sufficient memory in current region
                if to_cursor + copied_size > to_end {
                    (to_cursor, to_size) = to_iter.next().unwrap();
                    to_end = to_cursor + to_size;
                    to_cursor = align_allocation_no_fill::<VM>(to_cursor, align, offset);
                    debug_assert!(to_cursor + copied_size <= to_end);
                }
                // Get copied object
                let new_obj = VM::VMObjectModel::get_reference_when_copied_to(
                    obj,
                    to_cursor + Self::HEADER_RESERVED_IN_BYTES,
                );
                // SAFETY: `obj` was discovered in an allocated region of `self.pr` (so its
                // `HEADER_RESERVED_IN_BYTES` prefix is mapped, initialized, and `AtomicUsize`-aligned),
                // `_pass` enforces mutual exclusion with `compact` (`copy_to`), and all concurrent
                // forwarding-word reads use `AtomicUsize`.
                unsafe { Self::store_header_forwarding_pointer(obj, new_obj) };
                trace!(
                    "Calculate forward: {} (size when copied = {}) ~> {} (size = {})",
                    obj,
                    VM::VMObjectModel::get_size_when_copied(obj),
                    to_cursor,
                    copied_size
                );
                // bump to_cursor
                to_cursor += copied_size;
            }
        }
    }

    pub fn compact(&self) {
        let _pass = Lisp2PassGuard::acquire(self, true);
        let mut to = Address::ZERO;
        for (from_start, size) in self.pr.iterate_allocated_regions() {
            let from_end = from_start + size;
            for obj in self.linear_scan_objects(from_start..from_end) {
                let copied_size = VM::VMObjectModel::get_size_when_copied(obj);
                // clear the VO bit
                vo_bit::unset_vo_bit(obj);

                // SAFETY: `obj` is in an allocated region of `self.pr` with a mapped, initialized,
                // `AtomicUsize`-aligned forwarding header word, and `_pass` guarantees exclusive
                // compaction access (`pass_active`, `in_compact`, and `compact_rwlock`).
                let maybe_forwarding_pointer = unsafe { Self::get_header_forwarding_pointer(obj) };
                if let Some(forwarding_pointer) = maybe_forwarding_pointer {
                    trace!("Compact {} to {}", obj, forwarding_pointer);
                    let new_object = forwarding_pointer;
                    // SAFETY: `new_object` was placed inside an allocated region of `self.pr` at
                    // `to_cursor + Self::HEADER_RESERVED_IN_BYTES <= obj.to_object_start::<VM>() + Self::HEADER_RESERVED_IN_BYTES`,
                    // `obj`'s forwarding pointer has already been read above, and `_pass` guarantees
                    // exclusive compaction access to `new_object`'s `AtomicUsize`-aligned header word.
                    unsafe { Self::clear_header_forwarding_pointer(new_object) };

                    // copy object
                    trace!(" copy from {} to {}", obj, new_object);
                    let end_of_new_object =
                        VM::VMObjectModel::copy_to(obj, new_object, Address::ZERO);
                    // update VO bit,
                    vo_bit::set_vo_bit(new_object);
                    to = new_object.to_object_start::<VM>() + copied_size;
                    debug_assert_eq!(end_of_new_object, to);
                } else {
                    trace!("Skipping dead object {}", obj);
                }
            }
        }

        debug!("Compact end: to = {}", to);

        // reset the bump pointer
        self.pr.reset_cursor(to);
    }
}

struct Lisp2ObjectSize<VM>(std::marker::PhantomData<VM>);
impl<VM: VMBinding> crate::util::linear_scan::LinearScanObjectSize for Lisp2ObjectSize<VM> {
    fn size(object: ObjectReference) -> usize {
        VM::VMObjectModel::get_current_size(object)
    }
}
