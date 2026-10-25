use crate::util::address::Address;
use crate::util::freelist::FreeList;
use crate::util::opaque_pointer::*;
use std::sync::Mutex;

use super::layout::vm_layout::BYTES_IN_CHUNK;
use super::layout::VMMap;
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

    // XXX: In the original code reserve_pages & clear_request explicitly
    //      acquired a lock.
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

    /**
     * Commit pages to the page budget.  This is called after
     * successfully determining that the request can be satisfied by
     * both the page budget and virtual memory.  This simply accounts
     * for the discrepancy between <code>committed</code> and
     * <code>reserved</code> while the request was pending.
     *
     * This *MUST* be called by each PageResource during the
     * allocPages, and the caller must hold the lock.
     */
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

    /// Return the number of available physical pages by this resource. This includes all pages
    /// currently unused by this resource. If the resource is using a discontiguous space, it also
    /// includes the currently unassigned discontiguous space.
    ///
    /// Note: This just considers physical pages (i.e. virtual memory pages allocated for use by
    /// this resource). This calculation is orthogonal to and does not consider any restrictions on
    /// the number of pages this resource may actually use at any time (i.e. the number of
    /// committed and reserved pages).
    ///
    /// Note: The calculation is made on the assumption that all space that could be assigned to
    /// this resource would be assigned to this resource (i.e. the unused discontiguous space could
    /// just as likely be assigned to another competing resource).
    fn get_available_physical_pages(&self) -> usize;

    fn common(&self) -> &CommonPageResource;
    fn common_mut(&mut self) -> &mut CommonPageResource;
    fn vm_map(&self) -> &'static dyn VMMap {
        self.common().vm_map
    }

    // Some page resources need to record the start address.
    // This method will be called after the start address of the discontigous region is determined.
    // `start` is the computed start address.  By default, this does nothing.
    fn update_discontiguous_start(&mut self, _start: Address) {
        // Do nothing.
    }
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
    head_discontiguous_region: Mutex<Address>,
}

impl CommonPageResource {
    pub fn new(contiguous: bool, growable: bool, vm_map: &'static dyn VMMap) -> CommonPageResource {
        CommonPageResource {
            accounting: PageAccounting::new(),

            contiguous,
            growable,
            vm_map,

            head_discontiguous_region: Mutex::new(Address::ZERO),
        }
    }

    /// Extend the virtual memory associated with a particular discontiguous
    /// space.  This simply involves requesting a suitable number of chunks
    /// from the pool of chunks available to discontiguous spaces.
    ///
    /// If the concrete page resource is using a `FreeList`, it should pass it
    /// via `freelist`, or `None` if not using `FreeList`.
    pub fn grow_discontiguous_space(
        &self,
        space_descriptor: SpaceDescriptor,
        chunks: usize,
        freelist: Option<&mut dyn FreeList>,
    ) -> Address {
        let mut head_discontiguous_region = self.head_discontiguous_region.lock().unwrap();

        let new_head: Address = self.vm_map.allocate_contiguous_chunks(
            space_descriptor,
            chunks,
            *head_discontiguous_region,
            freelist,
        );
        if new_head.is_zero() {
            return Address::ZERO;
        }

        *head_discontiguous_region = new_head;
        new_head
    }

    /// Release one or more contiguous chunks associated with a discontiguous
    /// space.
    pub fn release_discontiguous_chunks(&self, chunk: Address) {
        assert!(
            !chunk.is_zero() && chunk.is_aligned_to(BYTES_IN_CHUNK),
            "chunk {chunk} must be non-zero and chunk-aligned"
        );
        let mut head_discontiguous_region = self.head_discontiguous_region.lock().unwrap();
        let mut cur = *head_discontiguous_region;
        let mut found = false;
        while !cur.is_zero() {
            if cur == chunk {
                found = true;
                break;
            }
            cur = self.vm_map.get_next_contiguous_region(cur);
        }
        assert!(
            found,
            "chunk {chunk} is not an allocated discontiguous region of this PageResource"
        );
        if chunk == *head_discontiguous_region {
            *head_discontiguous_region = self.vm_map.get_next_contiguous_region(chunk);
        }
        // SAFETY: `self.head_discontiguous_region` is a private `Mutex<Address>` whose linked list
        // only ever records non-zero, `BYTES_IN_CHUNK`-aligned region-start addresses returned by
        // `self.vm_map.allocate_contiguous_chunks` in `grow_discontiguous_space` (and removes them
        // here or in `release_all_chunks` under the same lock). Holding that lock and verifying
        // `assert!(found)` above proves `chunk` is a currently allocated region-start address in
        // `self.vm_map`.
        unsafe {
            self.vm_map.free_contiguous_chunks(chunk);
        }
    }

    /// Release all discontiguous chunk regions owned by this page resource back to the global pool.
    ///
    /// # Safety
    ///
    /// The caller must ensure that no live objects or active allocator cursors still reference any
    /// discontiguous chunks owned by this page resource, because releasing them clears their chunk
    /// descriptors and `SFT_MAP` entries and returns them to `self.vm_map` for reuse.
    pub unsafe fn release_all_chunks(&self) {
        let mut head_discontiguous_region = self.head_discontiguous_region.lock().unwrap();
        let head = *head_discontiguous_region;
        // SAFETY: `self.head_discontiguous_region` is a private `Mutex<Address>` initialized to
        // `Address::ZERO` and only updated in `grow_discontiguous_space` (to region-start addresses
        // returned by `self.vm_map.allocate_contiguous_chunks`), `release_discontiguous_chunks`,
        // and here under the same lock, so `head` and all regions linked from it are either empty
        // (`Address::ZERO`) or currently allocated region-start addresses of this space. The caller
        // guarantees no live objects or active allocator cursors reference any of those chunks.
        unsafe {
            self.vm_map.free_all_chunks(head);
        }
        *head_discontiguous_region = Address::ZERO;
    }

    pub fn get_head_discontiguous_region(&self) -> Address {
        *self.head_discontiguous_region.lock().unwrap()
    }
}
