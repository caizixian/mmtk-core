#![deny(unsafe_op_in_unsafe_fn)]

use crate::util::address::Address;
use crate::util::freelist::FreeList;
use crate::util::opaque_pointer::*;
use std::sync::Mutex;

use super::layout::vm_layout::BYTES_IN_CHUNK;
use super::layout::{AllocatedChunkRegion, VMMap};
use crate::util::heap::space_descriptor::SpaceDescriptor;
use crate::util::heap::PageAccounting;
use crate::vm::VMBinding;

pub trait PageResource<VM: VMBinding>: 'static {
    /// Allocate pages from this resource.
    /// Simply bump the cursor, and fail if we hit the sentinel.
    /// Return The start of the first page if successful, zero on failure.
    fn get_new_pages(
        &self,
        space_descriptor: SpaceDescriptor,
        reserved_pages: usize,
        required_pages: usize,
        tls: VMThread,
    ) -> Result<PRAllocResult, PRAllocFail> {
        self.alloc_pages(space_descriptor, reserved_pages, required_pages, tls)
    }

    fn reserve_pages(&self, pages: usize) -> usize {
        self.common().accounting.reserve(pages);
        pages
    }

    fn clear_request(&self, reserved_pages: usize) {
        self.common().accounting.clear_reserved(reserved_pages);
    }

    fn update_zeroing_approach(&self, _nontemporal: bool, concurrent: bool) {
        debug_assert!(!concurrent || self.common().contiguous);
        unimplemented!()
    }

    fn skip_concurrent_zeroing(&self) {
        unimplemented!()
    }

    fn trigger_concurrent_zeroing(&self) {
        unimplemented!()
    }

    fn concurrent_zeroing(&self) {
        panic!("This PageResource does not implement concurrent zeroing")
    }

    fn alloc_pages(
        &self,
        space_descriptor: SpaceDescriptor,
        reserved_pages: usize,
        required_pages: usize,
        tls: VMThread,
    ) -> Result<PRAllocResult, PRAllocFail>;

    /// Commit pages to the page budget. Called after determining that the request can be satisfied
    /// by both the page budget and virtual memory, while holding the page resource lock.
    fn commit_pages(&self, reserved_pages: usize, actual_pages: usize, _tls: VMThread) {
        let delta = actual_pages - reserved_pages;
        self.common().accounting.reserve(delta);
        self.common().accounting.commit(actual_pages);
    }

    fn reserved_pages(&self) -> usize {
        self.common().accounting.get_reserved_pages()
    }

    fn committed_pages(&self) -> usize {
        self.common().accounting.get_committed_pages()
    }

    /// Return the number of available physical pages by this resource.
    fn get_available_physical_pages(&self) -> usize;

    fn common(&self) -> &CommonPageResource;
    fn common_mut(&mut self) -> &mut CommonPageResource;
    fn vm_map(&self) -> &'static dyn VMMap {
        self.common().vm_map
    }

    fn update_discontiguous_start(&mut self, _start: Address) {}
}

pub struct PRAllocResult {
    pub start: Address,
    pub pages: usize,
    pub new_chunk: bool,
}

pub struct PRAllocFail;

pub struct CommonPageResource {
    pub accounting: PageAccounting,
    pub contiguous: bool,
    pub growable: bool,

    pub vm_map: &'static dyn VMMap,
    /// Owned chunk regions allocated via `grow_discontiguous_space` in chronological order
    /// (newest region at the back; reverse iteration yields newest-to-oldest order).
    discontiguous_regions: Mutex<Vec<AllocatedChunkRegion>>,
}

impl CommonPageResource {
    pub fn new(contiguous: bool, growable: bool, vm_map: &'static dyn VMMap) -> CommonPageResource {
        CommonPageResource {
            accounting: PageAccounting::new(),
            contiguous,
            growable,
            vm_map,
            discontiguous_regions: Mutex::new(Vec::new()),
        }
    }

    /// Extend the virtual memory associated with a discontiguous (or growable 64-bit) space.
    pub fn grow_discontiguous_space(
        &self,
        space_descriptor: SpaceDescriptor,
        chunks: usize,
        freelist: Option<&mut dyn FreeList>,
    ) -> Address {
        let mut regions = self.discontiguous_regions.lock().unwrap();
        let Some(region) =
            self.vm_map
                .allocate_contiguous_chunks(space_descriptor, chunks, freelist)
        else {
            return Address::ZERO;
        };
        let start = region.start();
        if !self.contiguous {
            regions.push(region);
        }
        start
    }

    /// Remove the owned discontiguous region starting at `chunk`, run `before_free(num_chunks)`,
    /// and return the region to `self.vm_map`.
    pub(crate) fn release_discontiguous_chunks_with(
        &self,
        chunk: Address,
        before_free: impl FnOnce(usize),
    ) {
        assert!(
            !chunk.is_zero() && chunk.is_aligned_to(BYTES_IN_CHUNK),
            "chunk {chunk} must be non-zero and chunk-aligned"
        );
        let region = {
            let mut regions = self.discontiguous_regions.lock().unwrap();
            let idx = regions
                .iter()
                .position(|r| r.start() == chunk)
                .unwrap_or_else(|| {
                    panic!(
                        "chunk {chunk} is not an allocated discontiguous region of this PageResource"
                    )
                });
            regions.remove(idx)
        };
        before_free(region.chunks());
        self.vm_map.free_contiguous_chunks(region);
    }

    /// Release all discontiguous chunk regions owned by this page resource back to the global pool.
    pub fn release_all_chunks(&self) {
        let drained: Vec<AllocatedChunkRegion> =
            std::mem::take(&mut *self.discontiguous_regions.lock().unwrap());
        for region in drained {
            self.vm_map.free_contiguous_chunks(region);
        }
    }

    /// Snapshot `(start, bytes)` for all owned discontiguous regions in newest-to-oldest order.
    pub(crate) fn discontiguous_region_ranges(&self) -> Vec<(Address, usize)> {
        let regions = self.discontiguous_regions.lock().unwrap();
        regions
            .iter()
            .rev()
            .map(|r| (r.start(), r.bytes()))
            .collect()
    }

    /// Truncate discontiguous regions in newest-to-oldest order after the region containing `top`
    /// (freeing older regions), returning `Some((current_chunk_start, current_chunk_end, live_bytes))`
    /// when at least one live region remains or `None` if no discontiguous regions are owned.
    pub(crate) fn truncate_discontiguous_regions_at(
        &self,
        mut top: Address,
    ) -> Option<(Address, Address, usize)> {
        let (result, to_free) = {
            let mut regions = self.discontiguous_regions.lock().unwrap();
            if regions.is_empty() {
                return None;
            }
            let mut live_size = 0usize;
            let mut found = None;
            let mut drain_end = 0usize;
            for (idx, region) in regions.iter().enumerate().rev() {
                let start = region.start();
                let end = start + region.bytes();
                if top.is_zero() {
                    top = start;
                }
                if top >= start && top < end {
                    live_size += top - start;
                    found = Some((start, end, live_size));
                    drain_end = idx;
                    break;
                }
                live_size += region.bytes();
            }
            let to_free: Vec<AllocatedChunkRegion> = regions.drain(0..drain_end).collect();
            (
                found.unwrap_or((Address::ZERO, Address::ZERO, live_size)),
                to_free,
            )
        };
        for region in to_free {
            self.vm_map.free_contiguous_chunks(region);
        }
        Some(result)
    }
}
