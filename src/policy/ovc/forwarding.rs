#![deny(unsafe_op_in_unsafe_fn)]

use crate::util::constants::BYTES_IN_WORD;
use crate::util::linear_scan::{Region, RegionIterator};
use crate::util::metadata::side_metadata::spec_defs::{OVC_MARK, OVC_OFFSET_VECTOR};
use crate::util::metadata::side_metadata::SideMetadataSpec;
use crate::util::{Address, ObjectReference};
use crate::vm::object_model::ObjectModel;
use crate::vm::VMBinding;
use atomic::Ordering;
use std::marker::PhantomData;
use std::sync::atomic::AtomicBool;

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

pub struct ForwardingMetadata<VM: VMBinding> {
    calculated: AtomicBool,
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

impl<VM: VMBinding> ForwardingMetadata<VM> {
    pub fn new() -> ForwardingMetadata<VM> {
        ForwardingMetadata {
            calculated: AtomicBool::new(false),
            vm: PhantomData,
        }
    }

    pub fn mark_last_word_of_object(&self, object: ObjectReference) {
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

    /// Populate `OFFSET_VECTOR_SPEC` across `[region.start(), cursor)` from quiescent `MARK_SPEC`.
    ///
    /// # Safety
    /// * `region` must be an allocated `OVCSpace` region with `MARK_SPEC` and `OFFSET_VECTOR_SPEC`
    ///   mapped and initialized across `[region.start(), region.end())`.
    /// * `cursor` must be aligned to `Block::BYTES` with `region.start() <= cursor <= region.end()`.
    /// * No thread may concurrently mutate `MARK_SPEC` in `[region.start(), cursor)` during this call.
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
            // SAFETY: Caller guarantees `block` lies within an allocated `OVCSpace` region whose
            // `MARK_SPEC` is mapped, initialized, and quiescent during `CalculateForwarding`, and
            // `state.visit_mark_bit` only updates local `Transducer` state.
            unsafe {
                MARK_SPEC.scan_non_zero_values::<u8>(
                    block.start(),
                    block.end(),
                    &mut |addr: Address| {
                        state.visit_mark_bit(addr);
                    },
                );
            }
        }
        self.calculated.store(true, Ordering::Release);
    }

    pub fn release(&self) {
        self.calculated.store(false, Ordering::Release);
    }

    /// Compute the forwarded destination address for an object at `address`.
    ///
    /// # Safety
    /// * `self.has_calculated_forwarding_addresses()` must be `true`, and `address` must reside in
    ///   an allocated `OVCSpace` region whose `MARK_SPEC` and `OFFSET_VECTOR_SPEC` are mapped and
    ///   initialized across `Block::from_unaligned_address(address)`.
    /// * No thread may concurrently mutate `MARK_SPEC` in `[block.start(), address)` during this call.
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
        // SAFETY: Caller guarantees `address` lies in a mapped `OVCSpace` block after offset-vector
        // calculation with quiescent `MARK_SPEC`, and `state.visit_mark_bit` does not mutate `MARK_SPEC`.
        unsafe {
            MARK_SPEC.scan_non_zero_values::<u8>(block.start(), address, &mut |addr: Address| {
                state.visit_mark_bit(addr)
            });
        }
        state.to
    }

    /// Scan start-of-object mark bits in `[start, end)` and invoke `f` on each marked object.
    ///
    /// # Safety
    /// * `[start, end)` must lie within an allocated `OVCSpace` region with `MARK_SPEC` mapped and
    ///   initialized.
    /// * Neither another thread nor `f` may mutate `MARK_SPEC` in `[start, end)` during this scan.
    pub(super) unsafe fn scan_marked_objects(
        &self,
        start: Address,
        end: Address,
        f: &mut impl FnMut(ObjectReference),
    ) {
        let mut in_object = false;
        // SAFETY: Caller guarantees `[start, end)` has mapped, initialized `MARK_SPEC` metadata and
        // neither another thread nor `f` mutates `MARK_SPEC` during the scan.
        unsafe {
            MARK_SPEC.scan_non_zero_values::<u8>(start, end, &mut |addr: Address| {
                if !in_object {
                    let object = ObjectReference::from_raw_address(addr).unwrap();
                    f(object);
                }
                in_object = !in_object;
            });
        }
    }

    pub fn has_calculated_forwarding_addresses(&self) -> bool {
        self.calculated.load(Ordering::Acquire)
    }
}
