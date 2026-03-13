//! Microbenchmark: AMAC (Asynchronous Memory Access Chaining) for GC tracing.
//!
//! # How to run
//!
//! ```sh
//! MMTK_BENCH=amac_tracing MMTK_PLAN=MarkSweep cargo bench \
//!     --features 'mock_test immortal_as_nonmoving'
//! ```
//!
//! # Background: AMAC (Kocberber et al., VLDB 2015)
//!
//! Traditional prefetching for pointer chasing uses a fixed lookahead distance:
//! "prefetch slot[i+D], then process slot[i]."  This is simple but rigid:
//!
//!   - All D in-flight lookups must be at the SAME pipeline stage
//!   - If one lookup finishes early (already marked → skip), its slot is wasted
//!   - Variable-length work (objects with different numbers of fields) causes
//!     pipeline bubbles
//!   - The distance D must be tuned per-workload; too short wastes MLP, too long
//!     wastes prefetch bandwidth
//!
//! AMAC takes a fundamentally different approach: maintain a circular buffer of
//! N independent "state machines," each tracking one object through the trace
//! pipeline.  States progress independently:
//!
//!   Stage 0 (SLOT_LOAD):  Prefetch issued for slot content.  Waiting.
//!   Stage 1 (OBJ_LOAD):   Slot loaded → got objref.  Prefetch issued for object header.
//!   Stage 2 (MARK_CHECK): Header loaded → check mark bit, CAS if needed.
//!   Stage 3 (SCAN):       Object newly marked → scan fields, produce child slots.
//!   Stage 4 (DONE):       This state slot is free for reuse.
//!
//! On each tick of the main loop:
//!   1. Advance the current state (circular index) by one step.
//!   2. If the state is DONE/FREE, refill it from the work queue (stage 0).
//!   3. Move to the next state slot (round-robin).
//!
//! Key advantages over fixed-distance prefetching:
//!   - **Dynamic**: each state progresses independently.  Already-marked objects
//!     free their slot immediately, which is then reused for a new lookup.
//!   - **Adaptive**: variable-length field scanning doesn't block other states.
//!   - **Maximal MLP**: always maintains exactly N in-flight memory accesses.
//!   - **No distance tuning**: the buffer size N replaces the prefetch distance D.
//!
//! # What this benchmark measures
//!
//! We compare five strategies on the same BFS graph tracing workload:
//!
//!   1. **Baseline**: sequential slot processing, no prefetching
//!   2. **Best-known prefetch**: combined edge+object, E=32/O=16 NTA
//!   3. **AMAC-4**: 4-slot circular buffer
//!   4. **AMAC-8**: 8-slot circular buffer
//!   5. **AMAC-16**: 16-slot circular buffer
//!   6. **AMAC-32**: 32-slot circular buffer
//!
//! We also test AMAC with a "three-stage" pipeline that additionally prefetches
//! the mark-bit metadata address (a separate memory region from the object),
//! which is something existing prefetching doesn't do.

use criterion::{black_box, Criterion};
use std::collections::VecDeque;
use std::time::Duration;

use rand::rngs::SmallRng;
use rand::RngExt;
use rand::SeedableRng;

use mmtk::memory_manager;
use mmtk::util::test_util::fixtures::*;
use mmtk::util::test_util::mock_method::*;
use mmtk::util::test_util::mock_vm::{write_mockvm, MockVM};
use mmtk::util::{Address, ObjectReference};
use mmtk::vm::slot::Slot;
use mmtk::AllocationSemantics;

const BYTES_PER_REF: usize = std::mem::size_of::<usize>();

// ───────────────────────────────────────────────────────────────────────────
// Object layout (identical to prefetch_tracing.rs for fair comparison)
// ───────────────────────────────────────────────────────────────────────────

const N_REFS: usize = 4;
const OBJ_PADDING: usize = 200;
const OBJ_SIZE: usize = 8 + 8 + 8 + N_REFS * BYTES_PER_REF + OBJ_PADDING;
const PACKET_SIZE: usize = 4096;

#[repr(C)]
struct KlassInfo {
    n_refs: usize,
    offsets: [usize; N_REFS],
}

#[inline(always)]
fn klass_ptr_addr(obj: ObjectReference) -> Address {
    obj.to_raw_address() + BYTES_PER_REF
}

#[inline(always)]
fn ref_field_addr(obj: ObjectReference, index: usize) -> Address {
    obj.to_raw_address() + 2 * BYTES_PER_REF + index * BYTES_PER_REF
}

#[inline(always)]
fn store_ref_field(obj: ObjectReference, index: usize, target: ObjectReference) {
    Slot::store(&ref_field_addr(obj, index), target);
}

#[inline(always)]
fn read_header(obj: ObjectReference) -> usize {
    unsafe { obj.to_raw_address().load::<usize>() }
}

#[inline(always)]
fn write_header(obj: ObjectReference, val: usize) {
    unsafe { obj.to_raw_address().store::<usize>(val) }
}

#[inline(always)]
fn load_klass(obj: ObjectReference) -> *const KlassInfo {
    unsafe { klass_ptr_addr(obj).load::<usize>() as *const KlassInfo }
}

fn clear_marks(objects: &[ObjectReference]) {
    for &obj in objects {
        let header = read_header(obj);
        write_header(obj, header & !1);
    }
}

fn allocate_objects(fixture: &mut MutatorFixture, count: usize) -> Vec<ObjectReference> {
    let mut objects = Vec::with_capacity(count);
    for _ in 0..count {
        let addr = memory_manager::alloc(
            &mut fixture.mutator,
            OBJ_SIZE,
            8,
            0,
            AllocationSemantics::Default,
        );
        assert!(!addr.is_zero(), "Allocation failed");
        let objref = MockVM::object_start_to_ref(addr);
        memory_manager::post_alloc(
            &mut fixture.mutator,
            objref,
            OBJ_SIZE,
            AllocationSemantics::Default,
        );
        objects.push(objref);
    }
    objects
}

fn build_random_dag_with_klass(
    objects: &[ObjectReference],
    klass_storage: &mut Vec<KlassInfo>,
) {
    let n = objects.len();
    let mut rng = SmallRng::seed_from_u64(0xdeadbeef12345678);

    let field_offsets: [usize; N_REFS] = std::array::from_fn(|i| {
        2 * BYTES_PER_REF + i * BYTES_PER_REF
    });

    klass_storage.clear();
    klass_storage.reserve(n);
    for _ in 0..n {
        klass_storage.push(KlassInfo {
            n_refs: N_REFS,
            offsets: field_offsets,
        });
    }

    for i in 0..n {
        let klass_ptr = &klass_storage[i] as *const KlassInfo as usize;
        unsafe {
            klass_ptr_addr(objects[i]).store::<usize>(klass_ptr);
        }
        for r in 0..N_REFS {
            let target = rng.random_range(0..n);
            store_ref_field(objects[i], r, objects[target]);
        }
    }
}

#[inline(always)]
fn prefetch_nta(addr: Address) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        std::arch::x86_64::_mm_prefetch(
            addr.to_ptr::<i8>(),
            std::arch::x86_64::_MM_HINT_NTA,
        );
    }
}

type BenchSlot = Address;

fn build_root_slots(objects: &[ObjectReference], n_roots: usize) -> Vec<BenchSlot> {
    let mut roots = Vec::with_capacity(n_roots * N_REFS);
    for i in 0..n_roots.min(objects.len()) {
        for r in 0..N_REFS {
            roots.push(ref_field_addr(objects[i], r));
        }
    }
    roots
}

// ───────────────────────────────────────────────────────────────────────────
// Strategy 1: Baseline (no prefetching)
// ───────────────────────────────────────────────────────────────────────────

#[inline(always)]
fn process_and_scan_slot(slot: BenchSlot, new_slots: &mut Vec<BenchSlot>) {
    let objref: Option<ObjectReference> = Slot::load(&slot);
    if let Some(obj) = objref {
        let header = read_header(obj);
        if header & 1 == 0 {
            write_header(obj, header | 1);
            let klass = load_klass(obj);
            let klass_info = unsafe { &*klass };
            for f in 0..klass_info.n_refs {
                let child_slot = obj.to_raw_address() + klass_info.offsets[f];
                new_slots.push(child_slot);
            }
        }
    }
}

fn trace_bfs_baseline(roots: &[BenchSlot], packet_size: usize) -> usize {
    let mut work_queue: VecDeque<Vec<BenchSlot>> = VecDeque::new();
    for chunk in roots.chunks(packet_size) {
        work_queue.push_back(chunk.to_vec());
    }
    let mut marked_count = 0usize;
    let mut new_slots = Vec::with_capacity(packet_size * 2);

    while let Some(packet) = work_queue.pop_front() {
        for i in 0..packet.len() {
            process_and_scan_slot(packet[i], &mut new_slots);
        }
        for chunk in new_slots.chunks(packet_size) {
            work_queue.push_back(chunk.to_vec());
        }
        marked_count += new_slots.len() / N_REFS;
        new_slots.clear();
    }
    marked_count
}

// ───────────────────────────────────────────────────────────────────────────
// Strategy 2: Best-known prefetch (E=32, O=16 NTA) — reference point
// ───────────────────────────────────────────────────────────────────────────

fn trace_bfs_prefetch_e32_o16(roots: &[BenchSlot], packet_size: usize) -> usize {
    let mut work_queue: VecDeque<Vec<BenchSlot>> = VecDeque::new();
    for chunk in roots.chunks(packet_size) {
        work_queue.push_back(chunk.to_vec());
    }
    let mut marked_count = 0usize;
    let mut new_slots = Vec::with_capacity(packet_size * 2);

    while let Some(packet) = work_queue.pop_front() {
        let len = packet.len();
        for i in 0..len {
            if i + 32 < len {
                prefetch_nta(packet[i + 32]);
            }
            if i + 16 < len {
                let future_objref: Option<ObjectReference> = Slot::load(&packet[i + 16]);
                if let Some(obj) = future_objref {
                    prefetch_nta(obj.to_raw_address());
                }
            }
            process_and_scan_slot(packet[i], &mut new_slots);
        }
        for chunk in new_slots.chunks(packet_size) {
            work_queue.push_back(chunk.to_vec());
        }
        marked_count += new_slots.len() / N_REFS;
        new_slots.clear();
    }
    marked_count
}

// ───────────────────────────────────────────────────────────────────────────
// Strategy 3: AMAC (Asynchronous Memory Access Chaining)
// ───────────────────────────────────────────────────────────────────────────
//
// The AMAC approach for GC tracing:
//
// We model the trace_object pipeline as a multi-stage state machine:
//
//   EMPTY → SLOT_PREFETCHED → OBJ_PREFETCHED → PROCESS → EMPTY
//
// Stage transitions:
//   EMPTY:            Pull next slot from work packet.  Issue prefetch for
//                     slot content.  → SLOT_PREFETCHED
//   SLOT_PREFETCHED:  Load slot → objref.  Issue prefetch for object header.
//                     → OBJ_PREFETCHED
//   OBJ_PREFETCHED:  Load header → check mark bit.  If already marked →
//                     EMPTY (slot freed instantly for reuse!).  Else mark
//                     and prepare to scan → PROCESS
//   PROCESS:         Scan object fields → push child slots to new_slots.
//                     → EMPTY
//
// The circular buffer has N slots, processed round-robin.  Each slot
// advances one stage per visit.  When a slot becomes EMPTY, it immediately
// pulls the next available work item.
//
// Key insight for GC: In the late phases of tracing, most objects are already
// marked.  With fixed-distance prefetching, these "early exits" are wasted
// pipeline slots.  With AMAC, the slot is immediately recycled for a new
// lookup, maintaining full MLP utilization.
//
// We also implement a variant that adds mark-bit metadata prefetching as a
// separate pipeline stage — something not done in Huang 2025 or existing work.

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum AmacStage {
    /// Slot is free — ready to accept new work
    Empty = 0,
    /// Prefetch issued for slot content, waiting for data
    SlotPrefetched = 1,
    /// Slot loaded, got objref.  Prefetch issued for object header.
    ObjPrefetched = 2,
    /// Header data ready — mark check + scan if needed
    ReadyToProcess = 3,
    /// Permanently done — no more work available for this slot
    Drained = 4,
}

#[derive(Clone, Copy)]
struct AmacState {
    stage: AmacStage,
    slot: BenchSlot,
    obj: ObjectReference,
}

impl AmacState {
    fn empty() -> Self {
        Self {
            stage: AmacStage::Empty,
            slot: unsafe { Address::from_usize(0) },
            // SAFETY: we never dereference this when stage is Empty
            obj: unsafe { ObjectReference::from_raw_address_unchecked(Address::from_usize(8)) },
        }
    }
}

fn make_amac_states<const N: usize>() -> [AmacState; N] {
    std::array::from_fn(|_| AmacState::empty())
}

/// AMAC-based BFS tracing with N concurrent pipeline slots.
///
/// The work queue feeds slots into the AMAC buffer.  We process one entire
/// work packet's worth of slots through the AMAC pipeline at a time, then
/// flush child slots into new work packets (matching the work-packet model).
fn trace_bfs_amac<const N: usize>(roots: &[BenchSlot], packet_size: usize) -> usize {
    let mut work_queue: VecDeque<Vec<BenchSlot>> = VecDeque::new();
    for chunk in roots.chunks(packet_size) {
        work_queue.push_back(chunk.to_vec());
    }
    let mut marked_count = 0usize;
    let mut new_slots = Vec::with_capacity(packet_size * 2);

    // The AMAC circular buffer
    let mut states = make_amac_states::<N>();

    while let Some(packet) = work_queue.pop_front() {
        let len = packet.len();
        let mut slot_cursor = 0usize; // next slot to pull from packet

        // Reset all states to Drained (permanently done)
        for s in states.iter_mut() {
            s.stage = AmacStage::Drained;
        }

        // Prime the pipeline: fill initial N slots
        let n_active = N.min(len);
        for s in 0..n_active {
            states[s].stage = AmacStage::SlotPrefetched;
            states[s].slot = packet[s];
            prefetch_nta(packet[s]);
            slot_cursor = s + 1;
        }

        // Main AMAC loop: round-robin through states until all done.
        // We loop until all N states are in Drained stage.
        let mut cursor = 0usize;
        let mut done_count = N - n_active; // un-primed states are already Drained

        while done_count < N {
            let s = &mut states[cursor];

            match s.stage {
                AmacStage::Empty => {
                    // Try to refill from packet
                    if slot_cursor < len {
                        s.slot = packet[slot_cursor];
                        prefetch_nta(packet[slot_cursor]);
                        s.stage = AmacStage::SlotPrefetched;
                        slot_cursor += 1;
                    } else {
                        // No more work — permanently drain this slot
                        s.stage = AmacStage::Drained;
                        done_count += 1;
                    }
                }
                AmacStage::SlotPrefetched => {
                    // Slot content should be in cache now.  Load objref.
                    let objref: Option<ObjectReference> = Slot::load(&s.slot);
                    if let Some(obj) = objref {
                        s.obj = obj;
                        // Prefetch object header (mark word + klass ptr)
                        prefetch_nta(obj.to_raw_address());
                        s.stage = AmacStage::ObjPrefetched;
                    } else {
                        // Null reference — free this slot
                        s.stage = AmacStage::Empty;
                    }
                }
                AmacStage::ObjPrefetched => {
                    // Object header should be in cache now.  Check mark bit.
                    let header = read_header(s.obj);
                    if header & 1 != 0 {
                        // Already marked — free slot immediately!
                        // This is where AMAC shines: the slot is recycled
                        // for a new lookup instead of being wasted.
                        s.stage = AmacStage::Empty;
                    } else {
                        // Newly marked
                        write_header(s.obj, header | 1);
                        s.stage = AmacStage::ReadyToProcess;
                    }
                }
                AmacStage::ReadyToProcess => {
                    // Scan object fields → produce child slots
                    let klass = load_klass(s.obj);
                    let klass_info = unsafe { &*klass };
                    for f in 0..klass_info.n_refs {
                        let child_slot = s.obj.to_raw_address() + klass_info.offsets[f];
                        new_slots.push(child_slot);
                    }
                    s.stage = AmacStage::Empty;
                }
                AmacStage::Drained => {
                    // This slot is permanently done — skip it
                }
            }

            cursor = (cursor + 1) % N;
        }

        // Flush child slots into new work packets
        for chunk in new_slots.chunks(packet_size) {
            work_queue.push_back(chunk.to_vec());
        }
        marked_count += new_slots.len() / N_REFS;
        new_slots.clear();
    }
    marked_count
}

// ───────────────────────────────────────────────────────────────────────────
// Strategy 4: AMAC with mark-bit metadata prefetch (three-stage)
// ───────────────────────────────────────────────────────────────────────────
//
// Extension of AMAC that adds a dedicated pipeline stage for prefetching
// the mark-bit metadata address.  In MMTk, marks are stored in side metadata
// (a separate contiguous memory region), NOT in the object header.
//
// With header-based marks:  object header load → read mark → CAS  (1 cache miss)
// With side metadata marks: object load → compute meta addr → load meta → CAS  (2 cache misses!)
//
// The side metadata address is computed as:
//   meta_addr = META_BASE + (data_addr >> log_bytes_in_region) >> shift
//
// This extra cache miss is a significant contributor to tracing latency.
// Neither Huang 2025 nor Atkinson 2023 prefetch the metadata address.
//
// Our three-stage AMAC pipeline:
//   EMPTY → SLOT_PREFETCHED → OBJ_PREFETCHED → META_PREFETCHED → PROCESS → EMPTY
//
// Stage META_PREFETCHED: compute mark-bit metadata address from the object
// address, issue prefetch for it.  Next visit: the metadata byte is in cache
// for the mark check.

/// Simulated side metadata: a separate byte array where mark bits live.
/// In real MMTk, this is `META_BASE + (addr >> log_region) >> shift`.
/// We allocate a region covering the full address range of objects,
/// indexed by (addr - base) >> log_granularity, simulating the cache-miss
/// behavior of real side metadata access to a separate memory region.
struct SideMetadata {
    /// Mark bytes indexed by (addr - base) >> shift
    data: Vec<u8>,
    /// Lowest object address
    base: usize,
    /// Right shift applied to address before indexing
    log_granularity: usize,
}

impl SideMetadata {
    fn new(objects: &[ObjectReference]) -> Self {
        if objects.is_empty() {
            return Self {
                data: Vec::new(),
                base: 0,
                log_granularity: 0,
            };
        }
        let min_addr = objects.iter().map(|o| o.to_raw_address().as_usize()).min().unwrap();
        let max_addr = objects.iter().map(|o| o.to_raw_address().as_usize()).max().unwrap();
        // Use 3-bit shift (8-byte granularity, same as real MMTk mark bits
        // which map 1 bit per MIN_OBJECT_SIZE=8 bytes)
        let log_granularity = 3;
        let size = ((max_addr - min_addr) >> log_granularity) + 1;
        Self {
            data: vec![0u8; size],
            base: min_addr,
            log_granularity,
        }
    }

    #[inline(always)]
    fn idx(&self, obj: ObjectReference) -> usize {
        (obj.to_raw_address().as_usize() - self.base) >> self.log_granularity
    }

    #[inline(always)]
    fn meta_addr(&self, obj: ObjectReference) -> *const u8 {
        let idx = self.idx(obj);
        &self.data[idx] as *const u8
    }

    #[inline(always)]
    fn meta_addr_mut(&mut self, obj: ObjectReference) -> *mut u8 {
        let idx = self.idx(obj);
        &mut self.data[idx] as *mut u8
    }

    #[inline(always)]
    fn is_marked(&self, obj: ObjectReference) -> bool {
        unsafe { *self.meta_addr(obj) != 0 }
    }

    #[inline(always)]
    fn set_mark(&mut self, obj: ObjectReference) {
        unsafe { *self.meta_addr_mut(obj) = 1; }
    }

    fn clear_all(&mut self) {
        for b in self.data.iter_mut() {
            *b = 0;
        }
    }

    #[inline(always)]
    fn prefetch_meta(&self, obj: ObjectReference) {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            std::arch::x86_64::_mm_prefetch(
                self.meta_addr(obj) as *const i8,
                std::arch::x86_64::_MM_HINT_NTA,
            );
        }
    }
}

/// Process a slot using side-metadata marks (simulates real MMTk marking).
#[inline(always)]
fn process_and_scan_slot_sidemeta(
    slot: BenchSlot,
    meta: &mut SideMetadata,
    new_slots: &mut Vec<BenchSlot>,
) {
    let objref: Option<ObjectReference> = Slot::load(&slot);
    if let Some(obj) = objref {
        if !meta.is_marked(obj) {
            meta.set_mark(obj);
            let klass = load_klass(obj);
            let klass_info = unsafe { &*klass };
            for f in 0..klass_info.n_refs {
                let child_slot = obj.to_raw_address() + klass_info.offsets[f];
                new_slots.push(child_slot);
            }
        }
    }
}

/// Baseline with side-metadata marks (no prefetching).
fn trace_bfs_sidemeta_baseline(
    roots: &[BenchSlot],
    meta: &mut SideMetadata,
    packet_size: usize,
) -> usize {
    let mut work_queue: VecDeque<Vec<BenchSlot>> = VecDeque::new();
    for chunk in roots.chunks(packet_size) {
        work_queue.push_back(chunk.to_vec());
    }
    let mut marked_count = 0usize;
    let mut new_slots = Vec::with_capacity(packet_size * 2);

    while let Some(packet) = work_queue.pop_front() {
        for i in 0..packet.len() {
            process_and_scan_slot_sidemeta(packet[i], meta, &mut new_slots);
        }
        for chunk in new_slots.chunks(packet_size) {
            work_queue.push_back(chunk.to_vec());
        }
        marked_count += new_slots.len() / N_REFS;
        new_slots.clear();
    }
    marked_count
}

/// Two-target prefetch with side metadata: prefetch both object AND meta.
fn trace_bfs_sidemeta_prefetch(
    roots: &[BenchSlot],
    meta: &mut SideMetadata,
    packet_size: usize,
) -> usize {
    let mut work_queue: VecDeque<Vec<BenchSlot>> = VecDeque::new();
    for chunk in roots.chunks(packet_size) {
        work_queue.push_back(chunk.to_vec());
    }
    let mut marked_count = 0usize;
    let mut new_slots = Vec::with_capacity(packet_size * 2);

    while let Some(packet) = work_queue.pop_front() {
        let len = packet.len();
        for i in 0..len {
            // Edge prefetch at distance 32
            if i + 32 < len {
                prefetch_nta(packet[i + 32]);
            }
            // Object + metadata prefetch at distance 16
            if i + 16 < len {
                let future_objref: Option<ObjectReference> = Slot::load(&packet[i + 16]);
                if let Some(obj) = future_objref {
                    prefetch_nta(obj.to_raw_address());  // object header
                    meta.prefetch_meta(obj);              // side metadata
                }
            }
            process_and_scan_slot_sidemeta(packet[i], meta, &mut new_slots);
        }
        for chunk in new_slots.chunks(packet_size) {
            work_queue.push_back(chunk.to_vec());
        }
        marked_count += new_slots.len() / N_REFS;
        new_slots.clear();
    }
    marked_count
}

/// AMAC stages for the three-stage (meta-prefetch-aware) variant.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum AmacStage3 {
    Empty = 0,
    SlotPrefetched = 1,
    ObjPrefetched = 2,
    MetaPrefetched = 3,
    ReadyToProcess = 4,
    Drained = 5,
}

#[derive(Clone, Copy)]
struct AmacState3 {
    stage: AmacStage3,
    slot: BenchSlot,
    obj: ObjectReference,
}

impl AmacState3 {
    fn empty() -> Self {
        Self {
            stage: AmacStage3::Empty,
            slot: unsafe { Address::from_usize(0) },
            obj: unsafe { ObjectReference::from_raw_address_unchecked(Address::from_usize(8)) },
        }
    }
}

fn make_amac3_states<const N: usize>() -> [AmacState3; N] {
    std::array::from_fn(|_| AmacState3::empty())
}

/// AMAC with three-stage pipeline including side-metadata prefetch.
fn trace_bfs_amac_sidemeta<const N: usize>(
    roots: &[BenchSlot],
    meta: &mut SideMetadata,
    packet_size: usize,
) -> usize {
    let mut work_queue: VecDeque<Vec<BenchSlot>> = VecDeque::new();
    for chunk in roots.chunks(packet_size) {
        work_queue.push_back(chunk.to_vec());
    }
    let mut marked_count = 0usize;
    let mut new_slots = Vec::with_capacity(packet_size * 2);

    let mut states = make_amac3_states::<N>();

    while let Some(packet) = work_queue.pop_front() {
        let len = packet.len();
        let mut slot_cursor = 0usize;

        // Reset all states to Drained
        for s in states.iter_mut() {
            s.stage = AmacStage3::Drained;
        }

        // Prime pipeline
        let n_active = N.min(len);
        for s in 0..n_active {
            states[s].stage = AmacStage3::SlotPrefetched;
            states[s].slot = packet[s];
            prefetch_nta(packet[s]);
            slot_cursor = s + 1;
        }

        let mut cursor = 0usize;
        let mut done_count = N - n_active;

        while done_count < N {
            let s = &mut states[cursor];

            match s.stage {
                AmacStage3::Empty => {
                    if slot_cursor < len {
                        s.slot = packet[slot_cursor];
                        prefetch_nta(packet[slot_cursor]);
                        s.stage = AmacStage3::SlotPrefetched;
                        slot_cursor += 1;
                    } else {
                        s.stage = AmacStage3::Drained;
                        done_count += 1;
                    }
                }
                AmacStage3::SlotPrefetched => {
                    let objref: Option<ObjectReference> = Slot::load(&s.slot);
                    if let Some(obj) = objref {
                        s.obj = obj;
                        prefetch_nta(obj.to_raw_address());
                        s.stage = AmacStage3::ObjPrefetched;
                    } else {
                        s.stage = AmacStage3::Empty;
                    }
                }
                AmacStage3::ObjPrefetched => {
                    meta.prefetch_meta(s.obj);
                    s.stage = AmacStage3::MetaPrefetched;
                }
                AmacStage3::MetaPrefetched => {
                    if meta.is_marked(s.obj) {
                        s.stage = AmacStage3::Empty;
                    } else {
                        meta.set_mark(s.obj);
                        s.stage = AmacStage3::ReadyToProcess;
                    }
                }
                AmacStage3::ReadyToProcess => {
                    let klass = load_klass(s.obj);
                    let klass_info = unsafe { &*klass };
                    for f in 0..klass_info.n_refs {
                        let child_slot = s.obj.to_raw_address() + klass_info.offsets[f];
                        new_slots.push(child_slot);
                    }
                    s.stage = AmacStage3::Empty;
                }
                AmacStage3::Drained => {
                    // Permanently done — skip
                }
            }

            cursor = (cursor + 1) % N;
        }

        for chunk in new_slots.chunks(packet_size) {
            work_queue.push_back(chunk.to_vec());
        }
        marked_count += new_slots.len() / N_REFS;
        new_slots.clear();
    }
    marked_count
}

// ───────────────────────────────────────────────────────────────────────────
// Strategy 5: Staged-Batch Pipeline (microarch-optimized AMAC)
// ───────────────────────────────────────────────────────────────────────────
//
// The original AMAC suffers from massive dispatch overhead:
//
//   Per state visit (from llvm-mca + uops.info on Zen 3):
//     - 12 µops for cursor advance + state lookup + jump table dispatch
//     - ~15 cycles indirect branch misprediction (5 match arms → ~60-80% mispredict)
//     - Total: ~25 cycles overhead per state visit
//     - With 3-4 visits per object × N objects → enormous overhead
//
//   Prefetch loop (for comparison):
//     - 3 µops for `inc; cmp; jb` (loop overhead)
//     - 0 cycles misprediction (well-predicted loop back-edge)
//
// The staged-batch approach eliminates ALL dispatch overhead by processing
// items in bulk through each pipeline stage:
//
//   Pass 1: Load N slot contents → N objrefs, issue N object prefetches
//   Pass 2: Check N mark bits, filter to newly-marked subset
//   Pass 3: Scan newly-marked objects → produce child slots
//
// Each pass is a tight sequential loop with no match/dispatch.  The inner
// loops compile to simple `inc; cmp; jb` back-edges (~0 misprediction).
// MLP is achieved because each pass issues all N loads before consuming
// the results in the next pass — the OOO engine overlaps them naturally.
//
// This is equivalent to the AMAC idea of having N independent accesses in
// flight, but without the round-robin state machine overhead.

fn trace_bfs_staged_batch<const N: usize>(
    roots: &[BenchSlot],
    packet_size: usize,
) -> usize {
    let mut work_queue: VecDeque<Vec<BenchSlot>> = VecDeque::new();
    for chunk in roots.chunks(packet_size) {
        work_queue.push_back(chunk.to_vec());
    }
    let mut marked_count = 0usize;
    let mut new_slots = Vec::with_capacity(packet_size * 2);

    // Scratch buffers for the pipeline stages (reused across packets)
    let mut objrefs: Vec<ObjectReference> = Vec::with_capacity(N);
    let mut to_scan: Vec<ObjectReference> = Vec::with_capacity(N);

    while let Some(packet) = work_queue.pop_front() {
        let len = packet.len();
        let mut cursor = 0usize;

        while cursor < len {
            let batch_end = (cursor + N).min(len);
            let batch = &packet[cursor..batch_end];

            // ── Pass 1: Load slots → objrefs, prefetch object headers ──
            objrefs.clear();
            for &slot in batch {
                let objref: Option<ObjectReference> = Slot::load(&slot);
                if let Some(obj) = objref {
                    // Issue prefetch for object header (mark word)
                    prefetch_nta(obj.to_raw_address());
                    objrefs.push(obj);
                }
            }

            // ── Pass 2: Check marks, filter to newly-marked ────────────
            to_scan.clear();
            for &obj in &objrefs {
                let header = read_header(obj);
                if header & 1 == 0 {
                    write_header(obj, header | 1);
                    to_scan.push(obj);
                }
            }

            // ── Pass 3: Scan newly-marked objects ──────────────────────
            for &obj in &to_scan {
                let klass = load_klass(obj);
                let klass_info = unsafe { &*klass };
                for f in 0..klass_info.n_refs {
                    let child_slot = obj.to_raw_address() + klass_info.offsets[f];
                    new_slots.push(child_slot);
                }
            }

            cursor = batch_end;
        }

        // Flush child slots into new work packets
        for chunk in new_slots.chunks(packet_size) {
            work_queue.push_back(chunk.to_vec());
        }
        marked_count += new_slots.len() / N_REFS;
        new_slots.clear();
    }
    marked_count
}

// ───────────────────────────────────────────────────────────────────────────
// Strategy 6: Interleaved Pipeline (software-pipelined, no dispatch)
// ───────────────────────────────────────────────────────────────────────────
//
// This variant manually interleaves DIFFERENT pipeline stages for
// consecutive slots in a single loop iteration:
//
//   for i in 0..len:
//     prefetch_edge(slot[i + 2*D])     // stage 0: prefetch far-ahead edge
//     prefetch_obj(load(slot[i + D]))  // stage 1: load slot D-ahead, prefetch its obj
//     process(slot[i])                 // stage 2: mark-check + scan current slot
//
// This is the same MLP as AMAC with D=distance, but with zero dispatch
// overhead — it's just a single flat loop with 3 inline operations.
// The "distance" D controls how far ahead we prefetch.
//
// The key difference from simple prefetching (Strategy 2) is that we
// also prefetch the edge content itself at distance 2*D, giving 3
// pipeline stages instead of 2.

fn trace_bfs_interleaved<const D: usize>(
    roots: &[BenchSlot],
    packet_size: usize,
) -> usize {
    let mut work_queue: VecDeque<Vec<BenchSlot>> = VecDeque::new();
    for chunk in roots.chunks(packet_size) {
        work_queue.push_back(chunk.to_vec());
    }
    let mut marked_count = 0usize;
    let mut new_slots = Vec::with_capacity(packet_size * 2);

    while let Some(packet) = work_queue.pop_front() {
        let len = packet.len();
        for i in 0..len {
            // Stage 0: Prefetch edge content at distance 2*D
            if i + 2 * D < len {
                prefetch_nta(packet[i + 2 * D]);
            }

            // Stage 1: Load slot at distance D, prefetch object header
            if i + D < len {
                let future_objref: Option<ObjectReference> = Slot::load(&packet[i + D]);
                if let Some(obj) = future_objref {
                    prefetch_nta(obj.to_raw_address());
                }
            }

            // Stage 2: Process current slot (mark check + scan)
            process_and_scan_slot(packet[i], &mut new_slots);
        }
        for chunk in new_slots.chunks(packet_size) {
            work_queue.push_back(chunk.to_vec());
        }
        marked_count += new_slots.len() / N_REFS;
        new_slots.clear();
    }
    marked_count
}

// ───────────────────────────────────────────────────────────────────────────
// Strategy 7: Staged-Batch with Side-Metadata Prefetch
// ───────────────────────────────────────────────────────────────────────────
//
// Combines the staged-batch idea with three-target prefetching for side
// metadata:
//
//   Pass 1: Load N slots → objrefs, prefetch object headers
//   Pass 2: Compute meta addresses, prefetch them
//   Pass 3: Check marks (both obj header and meta should be in cache)
//   Pass 4: Scan newly-marked objects

fn trace_bfs_staged_batch_sidemeta<const N: usize>(
    roots: &[BenchSlot],
    meta: &mut SideMetadata,
    packet_size: usize,
) -> usize {
    let mut work_queue: VecDeque<Vec<BenchSlot>> = VecDeque::new();
    for chunk in roots.chunks(packet_size) {
        work_queue.push_back(chunk.to_vec());
    }
    let mut marked_count = 0usize;
    let mut new_slots = Vec::with_capacity(packet_size * 2);

    let mut objrefs: Vec<ObjectReference> = Vec::with_capacity(N);
    let mut to_scan: Vec<ObjectReference> = Vec::with_capacity(N);

    while let Some(packet) = work_queue.pop_front() {
        let len = packet.len();
        let mut cursor = 0usize;

        while cursor < len {
            let batch_end = (cursor + N).min(len);
            let batch = &packet[cursor..batch_end];

            // ── Pass 1: Load slots → objrefs, prefetch headers ─────────
            objrefs.clear();
            for &slot in batch {
                let objref: Option<ObjectReference> = Slot::load(&slot);
                if let Some(obj) = objref {
                    prefetch_nta(obj.to_raw_address());
                    objrefs.push(obj);
                }
            }

            // ── Pass 2: Prefetch side-metadata for all objrefs ─────────
            for &obj in &objrefs {
                meta.prefetch_meta(obj);
            }

            // ── Pass 3: Check marks (header + meta in cache) ───────────
            to_scan.clear();
            for &obj in &objrefs {
                if !meta.is_marked(obj) {
                    meta.set_mark(obj);
                    to_scan.push(obj);
                }
            }

            // ── Pass 4: Scan newly-marked objects ──────────────────────
            for &obj in &to_scan {
                let klass = load_klass(obj);
                let klass_info = unsafe { &*klass };
                for f in 0..klass_info.n_refs {
                    let child_slot = obj.to_raw_address() + klass_info.offsets[f];
                    new_slots.push(child_slot);
                }
            }

            cursor = batch_end;
        }

        for chunk in new_slots.chunks(packet_size) {
            work_queue.push_back(chunk.to_vec());
        }
        marked_count += new_slots.len() / N_REFS;
        new_slots.clear();
    }
    marked_count
}

// ───────────────────────────────────────────────────────────────────────────
// Strategy 8: Interleaved Pipeline with Side-Metadata Prefetch
// ───────────────────────────────────────────────────────────────────────────
//
// Combines the interleaved pipeline (Strategy 6) with three-target
// prefetching for side-metadata marks:
//
//   for i in 0..len:
//     prefetch_edge(slot[i + 2*D])        // far-ahead: slot content
//     obj = load(slot[i + D])
//     prefetch_obj(obj)                   // mid-ahead: object header
//     prefetch_meta(meta_addr(obj))       // mid-ahead: mark-bit metadata
//     process_with_sidemeta(slot[i])      // current: mark-check + scan

fn trace_bfs_interleaved_sidemeta<const D: usize>(
    roots: &[BenchSlot],
    meta: &mut SideMetadata,
    packet_size: usize,
) -> usize {
    let mut work_queue: VecDeque<Vec<BenchSlot>> = VecDeque::new();
    for chunk in roots.chunks(packet_size) {
        work_queue.push_back(chunk.to_vec());
    }
    let mut marked_count = 0usize;
    let mut new_slots = Vec::with_capacity(packet_size * 2);

    while let Some(packet) = work_queue.pop_front() {
        let len = packet.len();
        for i in 0..len {
            // Stage 0: Prefetch edge content at distance 2*D
            if i + 2 * D < len {
                prefetch_nta(packet[i + 2 * D]);
            }

            // Stage 1: Load slot at distance D, prefetch object + metadata
            if i + D < len {
                let future_objref: Option<ObjectReference> = Slot::load(&packet[i + D]);
                if let Some(obj) = future_objref {
                    prefetch_nta(obj.to_raw_address());  // object header
                    meta.prefetch_meta(obj);              // side metadata
                }
            }

            // Stage 2: Process current slot with side-metadata marks
            let objref: Option<ObjectReference> = Slot::load(&packet[i]);
            if let Some(obj) = objref {
                if !meta.is_marked(obj) {
                    meta.set_mark(obj);
                    let klass = load_klass(obj);
                    let klass_info = unsafe { &*klass };
                    for f in 0..klass_info.n_refs {
                        let child_slot = obj.to_raw_address() + klass_info.offsets[f];
                        new_slots.push(child_slot);
                    }
                }
            }
        }
        for chunk in new_slots.chunks(packet_size) {
            work_queue.push_back(chunk.to_vec());
        }
        marked_count += new_slots.len() / N_REFS;
        new_slots.clear();
    }
    marked_count
}
// ───────────────────────────────────────────────────────────────────────────
// Correctness verification
// ───────────────────────────────────────────────────────────────────────────

fn verify_all(
    objects: &[ObjectReference],
    roots: &[BenchSlot],
    meta: &mut SideMetadata,
) {
    // Baseline (header marks)
    clear_marks(objects);
    let baseline_header = trace_bfs_baseline(roots, PACKET_SIZE);

    // Prefetch E32/O16 (header marks)
    clear_marks(objects);
    let prefetch_header = trace_bfs_prefetch_e32_o16(roots, PACKET_SIZE);

    // AMAC-8 (header marks)
    clear_marks(objects);
    let amac8_header = trace_bfs_amac::<8>(roots, PACKET_SIZE);

    // AMAC-16 (header marks)
    clear_marks(objects);
    let amac16_header = trace_bfs_amac::<16>(roots, PACKET_SIZE);

    // Staged-batch-16 (header marks)
    clear_marks(objects);
    let staged16_header = trace_bfs_staged_batch::<16>(roots, PACKET_SIZE);

    // Interleaved D=16 (header marks)
    clear_marks(objects);
    let interleaved_header = trace_bfs_interleaved::<16>(roots, PACKET_SIZE);

    // Side-metadata baseline
    meta.clear_all();
    let baseline_meta = trace_bfs_sidemeta_baseline(roots, meta, PACKET_SIZE);

    // Side-metadata prefetch
    meta.clear_all();
    let prefetch_meta = trace_bfs_sidemeta_prefetch(roots, meta, PACKET_SIZE);

    // AMAC sidemeta 16
    meta.clear_all();
    let amac16_meta = trace_bfs_amac_sidemeta::<16>(roots, meta, PACKET_SIZE);

    // Staged-batch sidemeta 16
    meta.clear_all();
    let staged16_meta = trace_bfs_staged_batch_sidemeta::<16>(roots, meta, PACKET_SIZE);

    // Interleaved sidemeta D=16
    meta.clear_all();
    let interleaved16_meta = trace_bfs_interleaved_sidemeta::<16>(roots, meta, PACKET_SIZE);

    assert_eq!(baseline_header, prefetch_header, "prefetch_header mismatch");
    assert_eq!(baseline_header, amac8_header, "amac8_header mismatch");
    assert_eq!(baseline_header, amac16_header, "amac16_header mismatch");
    assert_eq!(baseline_header, staged16_header, "staged16_header mismatch");
    assert_eq!(baseline_header, interleaved_header, "interleaved_header mismatch");
    assert_eq!(baseline_header, baseline_meta, "baseline_meta mismatch");
    assert_eq!(baseline_header, prefetch_meta, "prefetch_meta mismatch");
    assert_eq!(baseline_header, amac16_meta, "amac16_meta mismatch");
    assert_eq!(baseline_header, staged16_meta, "staged16_meta mismatch");
    assert_eq!(baseline_header, interleaved16_meta, "interleaved16_meta mismatch");

    eprintln!(
        "[amac_tracing] Correctness verified: all strategies mark {} objects",
        baseline_header
    );
    clear_marks(objects);
    meta.clear_all();
}

// ───────────────────────────────────────────────────────────────────────────
// Benchmark entry point
// ───────────────────────────────────────────────────────────────────────────

pub fn bench(c: &mut Criterion) {
    std::env::set_var("MMTK_PLAN", "MarkSweep");

    write_mockvm(|mock| {
        *mock = MockVM {
            is_collection_enabled: MockMethod::new_fixed(Box::new(|_| true)),
            get_object_size: MockMethod::new_fixed(Box::new(|_| OBJ_SIZE)),
            ..MockVM::default()
        };
    });

    let n_objects: usize = 4 << 20;
    let heap_size = n_objects * OBJ_SIZE * 2;

    eprintln!(
        "[amac_tracing] Allocating {} objects ({} MB, {} bytes/obj)...",
        n_objects,
        n_objects * OBJ_SIZE / (1 << 20),
        OBJ_SIZE,
    );

    let mut fixture = MutatorFixture::create_with_heapsize(heap_size);
    let objects = allocate_objects(&mut fixture, n_objects);
    let mut klass_storage = Vec::new();
    build_random_dag_with_klass(&objects, &mut klass_storage);

    let n_roots = 256;
    let root_slots = build_root_slots(&objects, n_roots);

    let mut side_meta = SideMetadata::new(&objects);

    eprintln!(
        "[amac_tracing] {} root slots, {} objects, side_meta={} KB",
        root_slots.len(),
        objects.len(),
        side_meta.data.len() / 1024,
    );

    verify_all(&objects, &root_slots, &mut side_meta);

    let measurement_time = Duration::from_secs(15);

    // ── Group 1: AMAC with header-based marks ───────────────────────────
    // Compare baseline, best-known prefetch, and AMAC at various buffer sizes.
    {
        let mut group = c.benchmark_group("amac_header_marks");
        group.measurement_time(measurement_time);

        group.bench_function("baseline", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_baseline(black_box(&root_slots), PACKET_SIZE))
            });
        });

        group.bench_function("prefetch_e32_o16", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_prefetch_e32_o16(
                    black_box(&root_slots), PACKET_SIZE,
                ))
            });
        });

        group.bench_function("amac_4", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_amac::<4>(black_box(&root_slots), PACKET_SIZE))
            });
        });

        group.bench_function("amac_8", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_amac::<8>(black_box(&root_slots), PACKET_SIZE))
            });
        });

        group.bench_function("amac_16", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_amac::<16>(black_box(&root_slots), PACKET_SIZE))
            });
        });

        group.bench_function("amac_32", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_amac::<32>(black_box(&root_slots), PACKET_SIZE))
            });
        });

        // ── Optimized: Staged-Batch (no dispatch overhead) ─────────────
        group.bench_function("staged_batch_8", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_staged_batch::<8>(black_box(&root_slots), PACKET_SIZE))
            });
        });

        group.bench_function("staged_batch_16", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_staged_batch::<16>(black_box(&root_slots), PACKET_SIZE))
            });
        });

        group.bench_function("staged_batch_32", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_staged_batch::<32>(black_box(&root_slots), PACKET_SIZE))
            });
        });

        // ── Optimized: Interleaved Pipeline (software-pipelined) ───────
        group.bench_function("interleaved_8", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_interleaved::<8>(black_box(&root_slots), PACKET_SIZE))
            });
        });

        group.bench_function("interleaved_16", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_interleaved::<16>(black_box(&root_slots), PACKET_SIZE))
            });
        });

        group.bench_function("interleaved_32", |b| {
            b.iter(|| {
                clear_marks(&objects);
                black_box(trace_bfs_interleaved::<32>(black_box(&root_slots), PACKET_SIZE))
            });
        });

        group.finish();
    }

    // ── Group 2: Side-metadata marks — baseline vs prefetch vs AMAC ─────
    // This group models real MMTk more accurately: marks are in a separate
    // memory region (side metadata), causing an additional cache miss.
    {
        let mut group = c.benchmark_group("amac_side_metadata");
        group.measurement_time(measurement_time);

        group.bench_function("sidemeta_baseline", |b| {
            b.iter(|| {
                side_meta.clear_all();
                black_box(trace_bfs_sidemeta_baseline(
                    black_box(&root_slots), &mut side_meta, PACKET_SIZE,
                ))
            });
        });

        group.bench_function("sidemeta_prefetch_e32_o16_meta", |b| {
            b.iter(|| {
                side_meta.clear_all();
                black_box(trace_bfs_sidemeta_prefetch(
                    black_box(&root_slots), &mut side_meta, PACKET_SIZE,
                ))
            });
        });

        group.bench_function("sidemeta_amac_4", |b| {
            b.iter(|| {
                side_meta.clear_all();
                black_box(trace_bfs_amac_sidemeta::<4>(
                    black_box(&root_slots), &mut side_meta, PACKET_SIZE,
                ))
            });
        });

        group.bench_function("sidemeta_amac_8", |b| {
            b.iter(|| {
                side_meta.clear_all();
                black_box(trace_bfs_amac_sidemeta::<8>(
                    black_box(&root_slots), &mut side_meta, PACKET_SIZE,
                ))
            });
        });

        group.bench_function("sidemeta_amac_16", |b| {
            b.iter(|| {
                side_meta.clear_all();
                black_box(trace_bfs_amac_sidemeta::<16>(
                    black_box(&root_slots), &mut side_meta, PACKET_SIZE,
                ))
            });
        });

        group.bench_function("sidemeta_amac_32", |b| {
            b.iter(|| {
                side_meta.clear_all();
                black_box(trace_bfs_amac_sidemeta::<32>(
                    black_box(&root_slots), &mut side_meta, PACKET_SIZE,
                ))
            });
        });

        // ── Optimized: Staged-Batch with side-metadata prefetch ────────
        group.bench_function("sidemeta_staged_batch_8", |b| {
            b.iter(|| {
                side_meta.clear_all();
                black_box(trace_bfs_staged_batch_sidemeta::<8>(
                    black_box(&root_slots), &mut side_meta, PACKET_SIZE,
                ))
            });
        });

        group.bench_function("sidemeta_staged_batch_16", |b| {
            b.iter(|| {
                side_meta.clear_all();
                black_box(trace_bfs_staged_batch_sidemeta::<16>(
                    black_box(&root_slots), &mut side_meta, PACKET_SIZE,
                ))
            });
        });

        group.bench_function("sidemeta_staged_batch_32", |b| {
            b.iter(|| {
                side_meta.clear_all();
                black_box(trace_bfs_staged_batch_sidemeta::<32>(
                    black_box(&root_slots), &mut side_meta, PACKET_SIZE,
                ))
            });
        });

        // ── Optimized: Interleaved with side-metadata prefetch ─────────
        group.bench_function("sidemeta_interleaved_8", |b| {
            b.iter(|| {
                side_meta.clear_all();
                black_box(trace_bfs_interleaved_sidemeta::<8>(
                    black_box(&root_slots), &mut side_meta, PACKET_SIZE,
                ))
            });
        });

        group.bench_function("sidemeta_interleaved_16", |b| {
            b.iter(|| {
                side_meta.clear_all();
                black_box(trace_bfs_interleaved_sidemeta::<16>(
                    black_box(&root_slots), &mut side_meta, PACKET_SIZE,
                ))
            });
        });

        group.bench_function("sidemeta_interleaved_32", |b| {
            b.iter(|| {
                side_meta.clear_all();
                black_box(trace_bfs_interleaved_sidemeta::<32>(
                    black_box(&root_slots), &mut side_meta, PACKET_SIZE,
                ))
            });
        });

        group.finish();
    }
}
