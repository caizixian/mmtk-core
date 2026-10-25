use super::vm_layout::{BYTES_IN_CHUNK, LOG_BYTES_IN_CHUNK};
use crate::util::freelist::FreeList;
use crate::util::heap::space_descriptor::SpaceDescriptor;
use crate::util::Address;

/// Linear (`!Copy`, `!Clone`) handle to a contiguous range of chunks allocated from a [`VMMap`].
///
/// Only minted by [`VMMap::allocate_contiguous_chunks`] and consumed by value in
/// [`VMMap::free_contiguous_chunks`], statically preventing double-frees or freeing unallocated /
/// interior chunk addresses.
#[derive(Debug)]
pub struct AllocatedChunkRegion {
    start: Address,
    chunks: u32,
}

impl AllocatedChunkRegion {
    pub(super) fn new(start: Address, chunks: usize) -> Self {
        assert!(
            !start.is_zero() && start.is_aligned_to(BYTES_IN_CHUNK),
            "AllocatedChunkRegion start ({start}) must be non-zero and chunk-aligned"
        );
        let chunks = u32::try_from(chunks).expect("chunk count exceeds u32::MAX");
        assert!(
            chunks > 0,
            "AllocatedChunkRegion must contain at least 1 chunk"
        );
        Self { start, chunks }
    }

    #[inline]
    pub(crate) fn start(&self) -> Address {
        self.start
    }

    #[inline]
    pub(crate) fn chunks(&self) -> usize {
        self.chunks as usize
    }

    #[inline]
    pub(crate) fn bytes(&self) -> usize {
        self.chunks() << LOG_BYTES_IN_CHUNK
    }
}

pub trait VMMap: Sync {
    fn insert(&self, start: Address, extent: usize, descriptor: SpaceDescriptor);

    /// Create a free-list for a discontiguous space. Must only be called at boot time once the
    /// discontiguous address range has been initialized by [`Self::finalize_static_space_map`].
    ///
    /// Requires `&'static self` because discontiguous child free lists (in `Map32`) store a
    /// shared reference to the global parent free list owned by `self`.
    fn create_freelist(&'static self, start: Address) -> Box<dyn FreeList>;

    /// Create a free-list for a contiguous space. Must only be called at boot time.
    fn create_parent_freelist(&self, start: Address, units: usize, grain: i32)
        -> Box<dyn FreeList>;

    /// Allocate `chunks` contiguous chunks for the space identified by `descriptor`.
    fn allocate_contiguous_chunks(
        &self,
        descriptor: SpaceDescriptor,
        chunks: usize,
        maybe_freelist: Option<&mut dyn FreeList>,
    ) -> Option<AllocatedChunkRegion>;

    /// Free an allocated contiguous region of chunks, returning the number of chunks freed.
    fn free_contiguous_chunks(&self, region: AllocatedChunkRegion) -> usize;

    /// Return the total number of chunks available (unassigned) within the range of virtual memory
    /// apportioned to discontiguous spaces.
    fn get_available_discontiguous_chunks(&self) -> usize;

    /// Return the total number of clients contending for chunks. This is useful when establishing
    /// conservative bounds on the number of remaining chunks.
    fn get_chunk_consumer_count(&self) -> usize;

    /// Finalize the global maps in the implementations of `VMMap`. This should be called after
    /// all spaces are created.
    fn finalize_static_space_map(
        &self,
        from: Address,
        to: Address,
        on_discontig_start_determined: &mut dyn FnMut(Address),
    );

    fn is_finalized(&self) -> bool;

    /// Get the space descriptor for the given address. Return `SpaceDescriptor::UNINITIALIZED` if
    /// the address is not within the MMTk heap range, or not within MMTk spaces.
    fn get_descriptor_for_address(&self, address: Address) -> SpaceDescriptor;

    /// Get the minimum extent of contiguous space that the `VMMap` implementation can support.
    fn min_contiguous_extent(&self) -> usize {
        BYTES_IN_CHUNK
    }
}
