use crate::util::constants::{BYTES_IN_PAGE, BYTES_IN_WORD, LOG_BITS_IN_BYTE, LOG_BYTES_IN_WORD};
use crate::util::heap::layout::vm_layout::BYTES_IN_CHUNK;
use crate::util::linear_scan::Region;
use crate::util::metadata::side_metadata::address_to_meta_address;
use crate::util::metadata::side_metadata::spec_defs::{OVC_MARK, OVC_OFFSET_VECTOR};
use crate::util::metadata::side_metadata::SideMetadataSpec;
use crate::util::{Address, ObjectReference};
use crate::vm::object_model::ObjectModel;
use crate::vm::VMBinding;
use atomic::Ordering;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize};
use std::sync::{Arc, Mutex};

/// A [`OVCRegion`] is the granularity at which [`super::OVCSpace`]
/// compacts the heap. Objects are allocated inside one region, and are only ever
/// moved *within* that region.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
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
    OVCRegion::BYTES <= BYTES_IN_CHUNK
        && MARK_BYTES_PER_BLOCK > 0
        && (MARK_BYTES_PER_BLOCK << LOG_DATA_BYTES_PER_MARK_BYTE) == Block::BYTES
);

fn assert_meta_range_mapped(meta_start: Address, meta_bytes: usize) {
    assert!(!meta_start.is_zero(), "Null OVC side-metadata address");
    assert!(
        meta_bytes <= isize::MAX as usize
            && meta_start.as_usize().checked_add(meta_bytes).is_some(),
        "OVC side-metadata range overflow ({meta_start} + 0x{meta_bytes:x})"
    );
    if meta_bytes == 0 {
        return;
    }
    let mut offset = 0usize;
    while offset < meta_bytes {
        assert!(
            (meta_start + offset).is_mapped(),
            "Unmapped OVC side-metadata page at {}",
            meta_start + offset
        );
        offset = offset.saturating_add(BYTES_IN_PAGE);
    }
    let last_byte = meta_start + (meta_bytes - 1);
    assert!(
        last_byte.is_mapped(),
        "Unmapped OVC side-metadata tail byte at {last_byte}"
    );
}

/// Validated `[region.start(), cursor)` prefix of an allocated [`OVCRegion`] together with its
/// permanently mapped atomic `OVC_MARK` and `OVC_OFFSET_VECTOR` side-metadata slices.
#[derive(Clone, Copy)]
pub(crate) struct MappedOVCRegion {
    region: OVCRegion,
    cursor: Address,
    mark_bytes: &'static [AtomicU8],
    offset_words: &'static [AtomicUsize],
}

impl MappedOVCRegion {
    /// Validate that `[region.start(), cursor)` is `Block`-aligned within a single chunk and that
    /// every backing `MARK_SPEC` and `OFFSET_VECTOR_SPEC` page is mapped, then project its atomic
    /// side-metadata slices.
    pub(super) fn from_allocated_prefix(region: OVCRegion, cursor: Address) -> Self {
        const _: () = assert!(
            OVCRegion::LOG_BYTES <= crate::util::heap::layout::vm_layout::LOG_BYTES_IN_CHUNK
        );
        let start = region.start();
        assert!(
            !start.is_zero()
                && start.is_aligned_to(OVCRegion::BYTES)
                && cursor >= start
                && cursor <= region.end()
                && cursor.is_aligned_to(Block::BYTES),
            "Invalid OVCRegion prefix [{start}, {cursor}) for region [{start}, {})",
            region.end()
        );
        let used_bytes = cursor - start;
        if used_bytes == 0 {
            return Self {
                region,
                cursor,
                mark_bytes: &[],
                offset_words: &[],
            };
        }

        assert!(
            !crate::util::metadata::side_metadata::global_side_metadata_base_address().is_zero(),
            "MappedOVCRegion::from_allocated_prefix called before side metadata initialization"
        );

        assert_eq!(
            start.align_down(BYTES_IN_CHUNK),
            (cursor - 1).align_down(BYTES_IN_CHUNK),
            "OVCRegion prefix [{start}, {cursor}) must reside within a single chunk"
        );
        let num_blocks = used_bytes >> Block::LOG_BYTES;
        let num_mark_bytes = num_blocks * MARK_BYTES_PER_BLOCK;
        let offset_byte_len = num_blocks
            .checked_mul(std::mem::size_of::<AtomicUsize>())
            .expect("OVC offset vector byte length overflow");

        let mark_meta = address_to_meta_address(&MARK_SPEC, start);
        let offset_meta = address_to_meta_address(&OFFSET_VECTOR_SPEC, start);
        if MARK_SPEC.uses_contiguous_side_metadata() {
            let spec_start = MARK_SPEC.get_starting_address();
            let spec_end = MARK_SPEC.upper_bound_address_for_contiguous();
            assert!(
                mark_meta >= spec_start
                    && mark_meta
                        .as_usize()
                        .checked_add(num_mark_bytes)
                        .is_some_and(|end| end <= spec_end.as_usize()),
                "OVC_MARK metadata [{mark_meta}, + {num_mark_bytes}) for [{start}, {cursor}) falls outside contiguous spec window [{spec_start}, {spec_end})"
            );
        }
        if OFFSET_VECTOR_SPEC.uses_contiguous_side_metadata() {
            let spec_start = OFFSET_VECTOR_SPEC.get_starting_address();
            let spec_end = OFFSET_VECTOR_SPEC.upper_bound_address_for_contiguous();
            assert!(
                offset_meta >= spec_start
                    && offset_meta
                        .as_usize()
                        .checked_add(offset_byte_len)
                        .is_some_and(|end| end <= spec_end.as_usize()),
                "OVC_OFFSET_VECTOR metadata [{offset_meta}, + {offset_byte_len}) for [{start}, {cursor}) falls outside contiguous spec window [{spec_start}, {spec_end})"
            );
        }
        assert!(
            offset_meta.is_aligned_to(std::mem::align_of::<AtomicUsize>()),
            "Misaligned OVC_OFFSET_VECTOR address {offset_meta}"
        );
        assert_meta_range_mapped(mark_meta, num_mark_bytes);
        assert_meta_range_mapped(offset_meta, offset_byte_len);

        // SAFETY:
        // 1. `start` is `OVCRegion::BYTES`-aligned (`1 MiB <= BYTES_IN_CHUNK`) and `cursor` is
        //    `Block::BYTES`-aligned within the same chunk, so `MARK_SPEC` (`1` bit per word) and
        //    `OFFSET_VECTOR_SPEC` (`1` `usize` per `Block`) map `[start, cursor)` to contiguous
        //    ranges `[mark_meta, mark_meta + num_mark_bytes)` and
        //    `[offset_meta, offset_meta + offset_byte_len)`.
        // 2. `assert_meta_range_mapped` verified both ranges are non-null, `<= isize::MAX`,
        //    non-wrapping, aligned (`AtomicU8` align 1; `AtomicUsize` checked above), and mapped
        //    across every OS page (`MMAPPER` maps zero-initialized memory monotonically and never
        //    unmaps chunks).
        // 3. `OVC_MARK` and `OVC_OFFSET_VECTOR` are OVC-private, non-overlapping local specs
        //    accessed across the entire crate exclusively through `AtomicU8` (`OVC_MARK`) and
        //    `AtomicUsize` (`OVC_OFFSET_VECTOR`), with zero non-atomic accesses or `&mut` aliases.
        let (mark_bytes, offset_words) = unsafe {
            (
                std::slice::from_raw_parts(mark_meta.to_ptr::<AtomicU8>(), num_mark_bytes),
                std::slice::from_raw_parts(offset_meta.to_ptr::<AtomicUsize>(), num_blocks),
            )
        };

        Self {
            region,
            cursor,
            mark_bytes,
            offset_words,
        }
    }

    #[inline]
    pub(super) fn region(&self) -> OVCRegion {
        self.region
    }

    #[cfg(feature = "vo_bit")]
    #[inline]
    pub(super) fn cursor(&self) -> Address {
        self.cursor
    }

    #[inline]
    pub(super) fn contains(&self, addr: Address) -> bool {
        addr >= self.region.start() && addr < self.cursor
    }

    /// Atomically clear all mark bits in `[self.region.start(), self.cursor)`.
    pub(super) fn clear_mark_bits(&self) {
        #[cfg(not(feature = "extreme_assertions"))]
        for cell in self.mark_bytes {
            cell.store(0, Ordering::Relaxed);
        }

        #[cfg(feature = "extreme_assertions")]
        for (byte_idx, cell) in self.mark_bytes.iter().enumerate() {
            let mut bits = cell.load(Ordering::Relaxed);
            if bits != 0 {
                let byte_data_base =
                    self.region.start() + (byte_idx << LOG_DATA_BYTES_PER_MARK_BYTE);
                while bits != 0 {
                    let bit = bits.trailing_zeros() as usize;
                    MARK_SPEC.store_atomic::<u8>(
                        byte_data_base + (bit << LOG_BYTES_IN_WORD),
                        0,
                        Ordering::Relaxed,
                    );
                    bits &= bits - 1;
                }
            }
        }
    }

    #[cfg(test)]
    fn clear_offset_vector(&self) {
        for word in self.offset_words {
            word.store(0, Ordering::Relaxed);
        }
    }

    #[inline]
    fn scan_mark_bits_in_block_prefix(
        &self,
        block_idx: usize,
        end: Address,
        visit: &mut impl FnMut(Address),
    ) {
        let block_start = self.region.start() + (block_idx << Block::LOG_BYTES);
        let block_end = block_start + Block::BYTES;
        assert!(
            end >= block_start && end <= block_end && end.is_aligned_to(BYTES_IN_WORD),
            "scan_mark_bits_in_block_prefix end ({end}) must be word-aligned in block [{block_start}, {block_end}]"
        );
        let words_before = (end - block_start) >> LOG_BYTES_IN_WORD;
        let full_bytes = words_before >> LOG_BITS_IN_BYTE;
        let rem_bits = (words_before & ((1usize << LOG_BITS_IN_BYTE) - 1)) as u32;
        let bytes_needed = full_bytes + usize::from(rem_bits > 0);

        let base_byte = block_idx * MARK_BYTES_PER_BLOCK;
        let block_marks = &self.mark_bytes[base_byte..base_byte + MARK_BYTES_PER_BLOCK];
        let mut snapshot = [0u8; MARK_BYTES_PER_BLOCK];
        for (dst, cell) in snapshot[..bytes_needed]
            .iter_mut()
            .zip(block_marks[..bytes_needed].iter())
        {
            *dst = cell.load(Ordering::Relaxed);
        }

        for (byte_idx, &byte) in snapshot[..full_bytes].iter().enumerate() {
            let mut bits = byte;
            let byte_data_base = block_start + (byte_idx << LOG_DATA_BYTES_PER_MARK_BYTE);
            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                visit(byte_data_base + (bit << LOG_BYTES_IN_WORD));
                bits &= bits - 1;
            }
        }

        if rem_bits > 0 {
            let mut bits = snapshot[full_bytes] & ((1u8 << rem_bits) - 1);
            let byte_data_base = block_start + (full_bytes << LOG_DATA_BYTES_PER_MARK_BYTE);
            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                visit(byte_data_base + (bit << LOG_BYTES_IN_WORD));
                bits &= bits - 1;
            }
        }
    }

    /// Populate `OFFSET_VECTOR_SPEC` across `[self.region.start(), self.cursor)` from `MARK_SPEC`.
    pub(super) fn calculate_offset_vector(&self) {
        let mut state = Transducer::new(self.region.start());
        for (block_idx, offset_slot) in self.offset_words.iter().enumerate() {
            let block_start = self.region.start() + (block_idx << Block::LOG_BYTES);
            offset_slot.store(state.encode(self.region, block_start), Ordering::Relaxed);
            self.scan_mark_bits_in_block_prefix(
                block_idx,
                block_start + Block::BYTES,
                &mut |addr| state.visit_mark_bit(addr),
            );
        }
    }

    /// Compute the forwarded destination address for a word-aligned `address` in `[start, cursor)`.
    #[inline]
    pub(super) fn forward(&self, address: Address) -> Address {
        assert!(
            self.contains(address) && address.is_aligned_to(BYTES_IN_WORD),
            "Address {address} is not word-aligned inside mapped OVCRegion [{}, {})",
            self.region.start(),
            self.cursor
        );
        let block_idx = (address - self.region.start()) >> Block::LOG_BYTES;
        let block_start = self.region.start() + (block_idx << Block::LOG_BYTES);
        let mut state = Transducer::decode(
            self.offset_words[block_idx].load(Ordering::Relaxed),
            self.region,
            block_start,
        );
        // The transducer in this implementation computes the final
        // address of an object; whereas Total-Live-Data in the paper computes
        // the distance of the object from the start of the block.
        self.scan_mark_bits_in_block_prefix(block_idx, address, &mut |addr| {
            state.visit_mark_bit(addr);
        });
        state.to
    }

    /// Scan start-of-object mark bits in `[self.region.start(), self.cursor)` and invoke `f` on
    /// each marked object.
    pub(super) fn scan_marked_objects(&self, f: &mut impl FnMut(ObjectReference)) {
        let mut in_object = false;
        for block_idx in 0..self.offset_words.len() {
            let block_end = self.region.start() + ((block_idx + 1) << Block::LOG_BYTES);
            self.scan_mark_bits_in_block_prefix(block_idx, block_end, &mut |addr| {
                if !in_object {
                    let object = ObjectReference::from_raw_address(addr).unwrap();
                    f(object);
                }
                in_object = !in_object;
            });
        }
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

/// Snapshot entry for one allocated region in the current GC cycle (`alloc_index` in
/// `RegionPageResource`, sorted by `mapped.region().start()` for $O(\log R)$ cross-region lookup).
#[derive(Clone, Copy)]
pub(super) struct CalculatedRegionEntry {
    pub(super) alloc_index: usize,
    pub(super) mapped: MappedOVCRegion,
}

pub(super) struct ForwardingMetadata<VM> {
    calculated: AtomicBool,
    pending_regions: AtomicUsize,
    calculated_regions: Mutex<Arc<[CalculatedRegionEntry]>>,
    vm: PhantomData<VM>,
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
            calculated_regions: Mutex::new(Arc::from(&[][..])),
            vm: PhantomData,
        }
    }

    /// Arm offset-vector calculation across `regions` (sorted in-place by `region.start()`).
    pub(super) fn begin_calculate(&self, mut regions: Vec<CalculatedRegionEntry>) {
        regions.sort_unstable_by_key(|entry| entry.mapped.region().start());
        let num_regions = regions.len();
        let mut guard = self.calculated_regions.lock().unwrap();
        self.calculated.store(false, Ordering::Release);
        *guard = Arc::from(regions);
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

    pub(super) fn reset(&self) {
        let mut guard = self.calculated_regions.lock().unwrap();
        self.calculated.store(false, Ordering::Release);
        self.pending_regions.store(0, Ordering::Release);
        *guard = Arc::from(&[][..]);
    }

    pub(super) fn release(&self) {
        self.reset();
    }

    /// Return a reference-counted snapshot of the sorted `CalculatedRegionEntry` slice if offset
    /// vectors have finished being calculated across all regions.
    pub(super) fn calculated_regions_if_ready(&self) -> Option<Arc<[CalculatedRegionEntry]>> {
        if !self.calculated.load(Ordering::Acquire) {
            return None;
        }
        let regions = Arc::clone(&self.calculated_regions.lock().unwrap());
        if !self.calculated.load(Ordering::Acquire) {
            return None;
        }
        Some(regions)
    }

    #[inline]
    pub(super) fn find_region(
        regions: &[CalculatedRegionEntry],
        addr: Address,
    ) -> Option<&MappedOVCRegion> {
        let region_start = addr.align_down(OVCRegion::BYTES);
        let idx = regions
            .binary_search_by_key(&region_start, |entry| entry.mapped.region().start())
            .ok()?;
        let mapped = &regions[idx].mapped;
        mapped.contains(addr).then_some(mapped)
    }

    #[inline]
    pub(super) fn forward_in_regions(
        regions: &[CalculatedRegionEntry],
        object: ObjectReference,
    ) -> ObjectReference {
        let addr = object.to_raw_address();
        assert!(
            addr.is_aligned_to(ObjectReference::ALIGNMENT) && addr.is_aligned_to(BYTES_IN_WORD),
            "Object {object} is not word/ObjectReference-aligned in OVCSpace"
        );
        let mapped = Self::find_region(regions, addr).unwrap_or_else(|| {
            panic!(
                "Object {object} is not within a mapped, calculated [region.start(), cursor) range of OVCSpace"
            )
        });
        ObjectReference::from_raw_address(mapped.forward(addr)).unwrap()
    }

    pub(super) fn has_calculated_forwarding_addresses(&self) -> bool {
        self.calculated.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mmap_anno_test;
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
            CalculatedRegionEntry {
                alloc_index: 0,
                mapped: MappedOVCRegion::from_allocated_prefix(r0, r0.start()),
            },
            CalculatedRegionEntry {
                alloc_index: 1,
                mapped: MappedOVCRegion::from_allocated_prefix(r1, r1.start()),
            },
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
            ctx.try_map_metadata_space(region_start, mapped_bytes, "OVCForwardingTest")
                .unwrap();
            let mapped = MappedOVCRegion::from_allocated_prefix(region, cursor);

            with_cleanup(
                || {
                    mapped.clear_mark_bits();
                    mapped.clear_offset_vector();

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
                    meta.begin_calculate(vec![CalculatedRegionEntry {
                        alloc_index: 0,
                        mapped,
                    }]);
                    mapped.calculate_offset_vector();
                    meta.finish_region_calculation();
                    assert!(meta.has_calculated_forwarding_addresses());

                    assert_eq!(mapped.forward(obj1), region_start);
                    assert_eq!(mapped.forward(obj2), region_start + 3 * BYTES_IN_WORD);
                    assert_eq!(mapped.forward(obj3), region_start + 8 * BYTES_IN_WORD);

                    let mut scanned = Vec::new();
                    mapped.scan_marked_objects(&mut |obj: ObjectReference| {
                        scanned.push(obj.to_raw_address());
                    });
                    assert_eq!(scanned, vec![obj1, obj2, obj3]);
                },
                || {
                    mapped.clear_mark_bits();
                    mapped.clear_offset_vector();
                },
            );
        });
    }
}
