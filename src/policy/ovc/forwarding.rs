#![deny(unsafe_op_in_unsafe_fn)]

use crate::util::constants::{BYTES_IN_WORD, LOG_BITS_IN_BYTE, LOG_BYTES_IN_WORD};
use crate::util::linear_scan::{Region, RegionIterator};
use crate::util::metadata::side_metadata::spec_defs::{OVC_MARK, OVC_OFFSET_VECTOR};
use crate::util::metadata::side_metadata::SideMetadataSpec;
use crate::util::{Address, ObjectReference};
use crate::vm::object_model::ObjectModel;
use crate::vm::VMBinding;
use atomic::Ordering;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{RwLock, RwLockReadGuard};

/// A [`OVCRegion`] is the granularity at which [`super::OVCSpace`]
/// compacts the heap. Objects are allocated inside one region, and are only ever
/// moved *within* that region.
#[derive(Copy, Clone, PartialEq, PartialOrd)]
pub(crate) struct OVCRegion(Address);
impl Region for OVCRegion {
    const LOG_BYTES: usize = 20; // 1 MiB
    fn from_aligned_address(address: Address) -> Self {
        Self::assert_aligned_non_zero(address);
        OVCRegion(address)
    }
    fn start(&self) -> Address {
        self.0
    }
}

/// A finite-state machine which visits the positions of marked bits in
/// the mark bitmap, and accumulates the size of live data that it has
/// seen between marked bits.
///
/// The Compressor caches the state of the transducer at the start of
/// each block by serialising the state using [`Transducer::encode`], and
/// then deserialises the state whilst computing forwarding pointers
/// using [`Transducer::decode`].
#[derive(Debug)]
struct Transducer {
    /// The address for the next object to be copied to, following preceding
    /// objects which were visited by the transducer.
    to: Address,
    /// The address of the last mark bit which the transducer visited.
    last_bit_visited: Address,
    /// Whether or not the transducer is currently inside an object
    /// (i.e. if it has seen a first bit but no matching last bit yet).
    in_object: bool,
}
impl Transducer {
    pub fn new(to: Address) -> Self {
        Self {
            to,
            last_bit_visited: Address::ZERO,
            in_object: false,
        }
    }
    pub fn visit_mark_bit(&mut self, address: Address) {
        if self.in_object {
            // The size of an object is the distance between the end and
            // start of the object, and the last word of the object is one
            // word prior to the end of the object. Thus we must add an
            // extra word, in order to compute the size of the object based
            // on the distance between its first and last words.
            let first_word = self.last_bit_visited;
            let last_word = address;
            let size = last_word - first_word + BYTES_IN_WORD;
            self.to += size;
        }
        self.in_object = !self.in_object;
        self.last_bit_visited = address;
    }

    pub fn encode(&self, region: OVCRegion, current_position: Address) -> usize {
        let to_offset_in_region = self.to - region.start();
        if self.in_object {
            // We count the space between the last mark bit and
            // the current address as live when we stop in the
            // middle of an object.
            to_offset_in_region + (current_position - self.last_bit_visited) + 1
        } else {
            to_offset_in_region
        }
    }

    pub fn decode(offset: usize, region: OVCRegion, current_position: Address) -> Self {
        Transducer {
            to: region.start() + (offset & !1),
            last_bit_visited: current_position,
            in_object: (offset & 1) == 1,
        }
    }
}

pub(super) struct ForwardingMetadata<VM> {
    calculated: AtomicBool,
    pending_regions: AtomicUsize,
    /// Pre-compaction `(region, cursor)` ranges for the current offset-vector pass.
    /// Cold readers (`get_forwarded_object`, `trace_forward_root`, `after_compact`,
    /// `debug_print_object_info`, `is_live`) hold a read guard to validate `[region.start(), cursor)`
    /// (which stays valid even after `Compact` lowers `AllocatedRegion::cursor` via `reset_cursor`),
    /// while hot `update_references` and `compact_region` avoid locking.
    calculated_regions: RwLock<Vec<(OVCRegion, Address)>>,
    vm: PhantomData<VM>,
}

// A block in the Compressor is the granularity at which we cache
// the amount of live data preceding an address. We set it to 512 bytes
// following the paper.
#[derive(Copy, Clone, PartialEq, PartialOrd)]
pub(crate) struct Block(Address);
impl Region for Block {
    const LOG_BYTES: usize = 9;
    fn from_aligned_address(address: Address) -> Self {
        Self::assert_aligned_non_zero(address);
        Block(address)
    }
    fn start(&self) -> Address {
        self.0
    }
}

pub(crate) const MARK_SPEC: SideMetadataSpec = OVC_MARK;
pub(crate) const OFFSET_VECTOR_SPEC: SideMetadataSpec = OVC_OFFSET_VECTOR;

const LOG_DATA_BYTES_PER_MARK_BYTE: usize =
    (LOG_BYTES_IN_WORD as usize) + (LOG_BITS_IN_BYTE as usize);
const MARK_BYTES_PER_BLOCK: usize = Block::BYTES >> LOG_DATA_BYTES_PER_MARK_BYTE;
const _: () = assert!(
    MARK_BYTES_PER_BLOCK > 0
        && (MARK_BYTES_PER_BLOCK << LOG_DATA_BYTES_PER_MARK_BYTE) == Block::BYTES
);

/// Snapshot `MARK_SPEC` across `[block.start(), end)` (`block.start() <= end <= block.end()`) via
/// `AtomicU8::load(Ordering::Relaxed)` and invoke `visit(word_addr)` for each set mark bit.
///
/// # Safety
/// * `block` must lie within an allocated `OVCSpace` region whose `MARK_SPEC` side metadata is
///   mapped and initialized across `[block.start(), block.end())`.
/// * No thread may concurrently perform non-atomic accesses (such as `OVCSpace::prepare`'s
///   `MARK_SPEC.bzero_metadata`) or hold live non-atomic slice references (`&[u8]` / `&mut [u8]`)
///   aliasing `block`'s `MARK_BYTES_PER_BLOCK` side-metadata bytes during the atomic snapshot.
#[inline]
unsafe fn scan_mark_bits_in_block_prefix(
    block: Block,
    end: Address,
    visit: &mut impl FnMut(Address),
) {
    assert!(
        end >= block.start() && end <= block.end() && end.is_aligned_to(BYTES_IN_WORD),
        "scan_mark_bits_in_block_prefix end ({}) must be word-aligned in block [{}, {}]",
        end,
        block.start(),
        block.end()
    );
    let words_before = (end - block.start()) >> LOG_BYTES_IN_WORD;
    let full_bytes = words_before >> LOG_BITS_IN_BYTE;
    let rem_bits = (words_before & ((1usize << LOG_BITS_IN_BYTE) - 1)) as u32;
    let bytes_needed = full_bytes + usize::from(rem_bits > 0);

    let mut snapshot = [0u8; MARK_BYTES_PER_BLOCK];
    {
        // SAFETY: `Block::BYTES` (`512`) is an exact multiple of `1 << LOG_DATA_BYTES_PER_MARK_BYTE`
        // (`64` on 64-bit, `32` on 32-bit) and lies within a single `BYTES_IN_CHUNK` (`4 MiB`), so
        // `block.start()` starts on an exact `MARK_SPEC` byte boundary within one chunk; caller
        // guarantees `[block.start(), block.end())` has mapped, initialized `MARK_SPEC` metadata
        // with no concurrent non-atomic accesses or non-atomic slice aliases. The `&[AtomicU8; _]`
        // borrow ends before `visit` runs.
        let mark_bytes = unsafe {
            MARK_SPEC.mapped_atomic_bytes_unchecked::<MARK_BYTES_PER_BLOCK>(block.start())
        };
        for (dst, cell) in snapshot[..bytes_needed]
            .iter_mut()
            .zip(mark_bytes[..bytes_needed].iter())
        {
            *dst = cell.load(Ordering::Relaxed);
        }
    }

    for (byte_idx, &byte) in snapshot[..full_bytes].iter().enumerate() {
        let mut bits = byte;
        let byte_data_base = block.start() + (byte_idx << LOG_DATA_BYTES_PER_MARK_BYTE);
        while bits != 0 {
            let bit = bits.trailing_zeros() as usize;
            visit(byte_data_base + (bit << LOG_BYTES_IN_WORD));
            bits &= bits - 1;
        }
    }

    if rem_bits > 0 {
        let mut bits = snapshot[full_bytes] & ((1u8 << rem_bits) - 1);
        let byte_data_base = block.start() + (full_bytes << LOG_DATA_BYTES_PER_MARK_BYTE);
        while bits != 0 {
            let bit = bits.trailing_zeros() as usize;
            visit(byte_data_base + (bit << LOG_BYTES_IN_WORD));
            bits &= bits - 1;
        }
    }
}

impl<VM: VMBinding> ForwardingMetadata<VM> {
    pub(super) fn mark_last_word_of_object(&self, object: ObjectReference) {
        let last_word_of_object = object.to_object_start::<VM>()
            + VM::VMObjectModel::get_current_size(object)
            - BYTES_IN_WORD;
        #[cfg(debug_assertions)]
        {
            // We require to be able to iterate upon first and last bits in the
            // same bitmap. Therefore the first and last bits cannot be the
            // same, else we would only encounter one of the two bits.
            // This requirement implies that objects must be at least two words
            // large.
            debug_assert!(
                MARK_SPEC.are_different_metadata_bits(
                    object.to_object_start::<VM>(),
                    last_word_of_object
                ),
                "The first and last mark bits should be different bits."
            );
        }

        // We only mark the last word as input to computing forwarding
        // information, so relaxed consistency is okay.
        MARK_SPEC.fetch_or_atomic::<u8>(last_word_of_object, 1, Ordering::Relaxed);
    }
}

impl<VM> ForwardingMetadata<VM> {
    pub(super) fn new() -> ForwardingMetadata<VM> {
        ForwardingMetadata {
            calculated: AtomicBool::new(false),
            pending_regions: AtomicUsize::new(0),
            calculated_regions: RwLock::new(Vec::new()),
            vm: PhantomData,
        }
    }

    /// Arm offset-vector calculation across `regions` (`(region, pre_compact_cursor)` pairs).
    /// If `regions.is_empty()`, forwarding addresses are vacuously ready immediately; otherwise
    /// `calculated` stays `false` until all `regions.len()` calls to
    /// [`Self::finish_region_calculation`] complete.
    pub(super) fn begin_calculate(&self, regions: Vec<(OVCRegion, Address)>) {
        let mut guard = self.calculated_regions.write().unwrap();
        let num_regions = regions.len();
        self.calculated.store(false, Ordering::Release);
        *guard = regions;
        if num_regions == 0 {
            self.pending_regions.store(0, Ordering::Release);
            self.calculated.store(true, Ordering::Release);
        } else {
            self.pending_regions.store(num_regions, Ordering::Release);
        }
    }

    /// Record completion of one region's offset-vector calculation, publishing
    /// `calculated = true` (`Ordering::Release`) only when the final region completes.
    pub(super) fn finish_region_calculation(&self) {
        let prev = self.pending_regions.fetch_sub(1, Ordering::AcqRel);
        assert!(
            prev > 0,
            "ForwardingMetadata::finish_region_calculation underflow"
        );
        if prev == 1 {
            self.calculated.store(true, Ordering::Release);
        }
    }

    /// Clear `calculated`, `pending_regions`, and `calculated_regions` (called at the start of
    /// `prepare()` and in `release()`).
    pub(super) fn reset(&self) {
        let mut guard = self.calculated_regions.write().unwrap();
        self.calculated.store(false, Ordering::Release);
        self.pending_regions.store(0, Ordering::Release);
        guard.clear();
    }

    pub(super) fn release(&self) {
        self.reset();
    }

    /// If offset vectors have finished being calculated across all regions, return a read guard
    /// over the pre-compaction `(region, cursor)` ranges (keeping `calculated == true` and
    /// `calculated_regions` pinned against concurrent `reset`/`release` until dropped).
    pub(super) fn calculated_regions_if_ready(
        &self,
    ) -> Option<RwLockReadGuard<'_, Vec<(OVCRegion, Address)>>> {
        if !self.calculated.load(Ordering::Acquire) {
            return None;
        }
        let guard = self.calculated_regions.read().unwrap();
        if !self.calculated.load(Ordering::Acquire) {
            return None;
        }
        Some(guard)
    }

    /// Populate `OFFSET_VECTOR_SPEC` across `[region.start(), cursor)` from `MARK_SPEC`.
    ///
    /// # Safety
    /// * `region` must be an allocated `OVCSpace` region with `cursor` aligned to `Block::BYTES`
    ///   (`region.start() <= cursor <= region.end()`), and both `MARK_SPEC` and `OFFSET_VECTOR_SPEC`
    ///   must be mapped and initialized across `[region.start(), cursor)`.
    /// * No thread may concurrently perform non-atomic accesses (e.g. `OVCSpace::prepare`) or hold
    ///   non-atomic slice aliases to `MARK_SPEC` or `OFFSET_VECTOR_SPEC` in `[region.start(), cursor)`
    ///   during this call.
    pub(super) unsafe fn calculate_offset_vector(&self, region: OVCRegion, cursor: Address) {
        let mut state = Transducer::new(region.start());
        let first_block = Block::from_aligned_address(region.start());
        let last_block = Block::from_aligned_address(cursor);
        for block in RegionIterator::<Block>::new(first_block, last_block) {
            OFFSET_VECTOR_SPEC.store_atomic::<usize>(
                block.start(),
                state.encode(region, block.start()),
                Ordering::Relaxed,
            );
            // SAFETY: Caller guarantees `block` lies within an allocated `OVCSpace` region with
            // mapped, initialized `MARK_SPEC` and no concurrent non-atomic accesses (`prepare`).
            unsafe {
                scan_mark_bits_in_block_prefix(block, block.end(), &mut |addr: Address| {
                    state.visit_mark_bit(addr);
                });
            }
        }
    }

    /// Compute the forwarded destination address for an object at `address`.
    ///
    /// # Safety
    /// * `self.has_calculated_forwarding_addresses()` must be `true`, and `address` must reside in
    ///   an allocated `OVCSpace` region whose `MARK_SPEC` and `OFFSET_VECTOR_SPEC` are mapped and
    ///   initialized across `Block::from_unaligned_address(address)`.
    /// * No thread may concurrently perform non-atomic accesses (e.g. `OVCSpace::prepare`) or hold
    ///   non-atomic slice aliases to `MARK_SPEC` or `OFFSET_VECTOR_SPEC` in `block` during this call.
    pub(super) unsafe fn forward(&self, address: Address) -> Address {
        assert!(
            self.calculated.load(Ordering::Acquire),
            "forward() should only be called when we have calculated an offset vector"
        );
        let region = OVCRegion::from_unaligned_address(address);
        let block = Block::from_unaligned_address(address);
        let mut state = Transducer::decode(
            OFFSET_VECTOR_SPEC.load_atomic::<usize>(block.start(), Ordering::Relaxed),
            region,
            block.start(),
        );
        // The transducer in this implementation computes the final
        // address of an object; whereas Total-Live-Data in the paper computes
        // the distance of the object from the start of the block.
        // SAFETY: Caller guarantees `block` lies in an allocated `OVCSpace` region with mapped,
        // initialized `MARK_SPEC` and no concurrent non-atomic accesses (`prepare`).
        unsafe {
            scan_mark_bits_in_block_prefix(block, address, &mut |addr: Address| {
                state.visit_mark_bit(addr);
            });
        }
        state.to
    }

    /// Scan start-of-object mark bits in `Block`-aligned `[start, end)` and invoke `f` on each
    /// marked object.
    ///
    /// # Safety
    /// * `[start, end)` must be `Block`-aligned (`start <= end`) within an allocated `OVCSpace`
    ///   region with `MARK_SPEC` mapped and initialized.
    /// * No thread (nor `f`) may perform non-atomic accesses (e.g. `OVCSpace::prepare`) or hold
    ///   non-atomic slice aliases to `MARK_SPEC` in `[start, end)` during this scan.
    pub(super) unsafe fn scan_marked_objects(
        &self,
        start: Address,
        end: Address,
        f: &mut impl FnMut(ObjectReference),
    ) {
        assert!(
            start <= end && start.is_aligned_to(Block::BYTES) && end.is_aligned_to(Block::BYTES),
            "scan_marked_objects requires Block-aligned [start, end): [{}, {})",
            start,
            end
        );
        let first_block = Block::from_aligned_address(start);
        let last_block = Block::from_aligned_address(end);
        let mut in_object = false;
        for block in RegionIterator::<Block>::new(first_block, last_block) {
            // SAFETY: Caller guarantees `block` in `[start, end)` lies in an allocated `OVCSpace`
            // region with mapped, initialized `MARK_SPEC` and no concurrent non-atomic accesses
            // (`scan_mark_bits_in_block_prefix` drops its `&[AtomicU8; _]` borrow before `f` runs).
            unsafe {
                scan_mark_bits_in_block_prefix(block, block.end(), &mut |addr: Address| {
                    if !in_object {
                        let object = ObjectReference::from_raw_address(addr).unwrap();
                        f(object);
                    }
                    in_object = !in_object;
                });
            }
        }
    }

    pub(super) fn has_calculated_forwarding_addresses(&self) -> bool {
        self.calculated.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mmap_anno_test;
    use crate::util::constants::BYTES_IN_PAGE;
    use crate::util::heap::layout::vm_layout;
    use crate::util::metadata::side_metadata::{
        core_test_initialize_side_metadata, SideMetadataContext, SideMetadataSanity,
    };
    use crate::util::os::{HugePageSupport, MmapProtection};
    use crate::util::test_util::{serial_test, with_cleanup};

    #[test]
    fn test_forwarding_metadata_multi_region_countdown_and_reset() {
        let meta = ForwardingMetadata::<()>::new();
        assert!(!meta.has_calculated_forwarding_addresses());
        assert!(meta.calculated_regions_if_ready().is_none());

        meta.begin_calculate(Vec::new());
        assert!(meta.has_calculated_forwarding_addresses());
        assert!(meta.calculated_regions_if_ready().unwrap().is_empty());

        meta.reset();
        assert!(!meta.has_calculated_forwarding_addresses());
        assert!(meta.calculated_regions_if_ready().is_none());

        let base = vm_layout::vm_layout().heap_start.align_up(OVCRegion::BYTES);
        let r0 = OVCRegion::from_aligned_address(base);
        let r1 = OVCRegion::from_aligned_address(base + OVCRegion::BYTES);
        meta.begin_calculate(vec![
            (r0, base + BYTES_IN_PAGE),
            (r1, r1.start() + BYTES_IN_PAGE),
        ]);
        assert!(!meta.has_calculated_forwarding_addresses());
        assert!(meta.calculated_regions_if_ready().is_none());

        meta.finish_region_calculation();
        assert!(!meta.has_calculated_forwarding_addresses());
        assert!(meta.calculated_regions_if_ready().is_none());

        meta.finish_region_calculation();
        assert!(meta.has_calculated_forwarding_addresses());
        assert_eq!(meta.calculated_regions_if_ready().unwrap().len(), 2);

        meta.release();
        assert!(!meta.has_calculated_forwarding_addresses());
        assert!(meta.calculated_regions_if_ready().is_none());
    }

    #[test]
    fn test_calculate_offset_vector_forward_and_scan_across_blocks() {
        serial_test(|| {
            core_test_initialize_side_metadata();
            let region_start = vm_layout::vm_layout().heap_start.align_up(OVCRegion::BYTES);
            let mapped_bytes = 2 * BYTES_IN_PAGE;
            let cursor = region_start + mapped_bytes;
            let region = OVCRegion::from_aligned_address(region_start);
            let ctx = SideMetadataContext {
                global: vec![],
                local: vec![MARK_SPEC, OFFSET_VECTOR_SPEC],
            };
            let mut sanity = SideMetadataSanity::new();
            sanity.verify_metadata_context("OVCForwardingTest", &ctx);

            crate::MMAPPER
                .ensure_mapped(
                    region_start,
                    2,
                    HugePageSupport::No,
                    MmapProtection::ReadWrite,
                    mmap_anno_test!(),
                )
                .unwrap();

            with_cleanup(
                || {
                    ctx.try_map_metadata_space(region_start, mapped_bytes, "OVCForwardingTest")
                        .unwrap();
                    MARK_SPEC.bzero_metadata(region_start, mapped_bytes);
                    OFFSET_VECTOR_SPEC.bzero_metadata(region_start, mapped_bytes);

                    // Object 1 in Block 0: 3 words at [region_start + 2w, region_start + 5w)
                    let obj1 = region_start + 2 * BYTES_IN_WORD;
                    let obj1_last = region_start + 4 * BYTES_IN_WORD;
                    // Object 2 straddling Block 0 -> Block 1: 5 words at [block1 - 2w, block1 + 3w)
                    let block1 = region_start + Block::BYTES;
                    let obj2 = block1 - 2 * BYTES_IN_WORD;
                    let obj2_last = block1 + 2 * BYTES_IN_WORD;
                    // Object 3 in Block 1 (odd word offset in partial byte): 2 words at [block1 + 11w, block1 + 13w)
                    let obj3 = block1 + 11 * BYTES_IN_WORD;
                    let obj3_last = block1 + 12 * BYTES_IN_WORD;

                    for addr in [obj1, obj1_last, obj2, obj2_last, obj3, obj3_last] {
                        MARK_SPEC.store_atomic::<u8>(addr, 1, Ordering::Relaxed);
                    }

                    let meta = ForwardingMetadata::<()>::new();
                    meta.begin_calculate(vec![(region, cursor)]);
                    // SAFETY: `MARK_SPEC` and `OFFSET_VECTOR_SPEC` are mapped and initialized across
                    // `[region_start, cursor)` under exclusive single-threaded `serial_test`.
                    unsafe {
                        meta.calculate_offset_vector(region, cursor);
                    }
                    meta.finish_region_calculation();
                    assert!(meta.has_calculated_forwarding_addresses());

                    // SAFETY: Offset vector was calculated for `[region_start, cursor)` and both
                    // side-metadata specs are mapped with no concurrent accesses.
                    unsafe {
                        assert_eq!(meta.forward(obj1), region_start);
                        assert_eq!(meta.forward(obj2), region_start + 3 * BYTES_IN_WORD);
                        assert_eq!(meta.forward(obj3), region_start + 8 * BYTES_IN_WORD);
                    }

                    let mut scanned = Vec::new();
                    // SAFETY: `[region_start, cursor)` is `Block`-aligned with mapped `MARK_SPEC`.
                    unsafe {
                        meta.scan_marked_objects(
                            region_start,
                            cursor,
                            &mut |obj: ObjectReference| {
                                scanned.push(obj.to_raw_address());
                            },
                        );
                    }
                    assert_eq!(scanned, vec![obj1, obj2, obj3]);
                },
                || {
                    MARK_SPEC.bzero_metadata(region_start, mapped_bytes);
                    OFFSET_VECTOR_SPEC.bzero_metadata(region_start, mapped_bytes);
                },
            );
        });
    }
}
