#![deny(unsafe_op_in_unsafe_fn)]

use super::layout::vm_layout::{BYTES_IN_CHUNK, LOG_BYTES_IN_CHUNK, PAGES_IN_CHUNK};
use super::layout::VMMap;
use super::pageresource::{CommonPageResource, PRAllocFail, PRAllocResult};
use super::PageResource;
use crate::policy::space::required_chunks;
use crate::util::address::Address;
use crate::util::constants::BYTES_IN_PAGE;
use crate::util::conversions::*;
use crate::util::heap::space_descriptor::SpaceDescriptor;
use crate::util::opaque_pointer::*;
use crate::vm::VMBinding;
use std::marker::PhantomData;
use std::sync::{Mutex, MutexGuard};

pub struct MonotonePageResource<VM: VMBinding> {
    common: CommonPageResource,
    sync: Mutex<MonotonePageResourceSync>,
    _p: PhantomData<VM>,
}

struct MonotonePageResourceSync {
    /// Pointer to the next block to be allocated.
    cursor: Address,
    /// The limit of the currently allocated address space.
    sentinel: Address,
    /// Base address of the current chunk of addresses.
    current_chunk: Address,
    conditional: MonotonePageResourceConditional,
}

pub enum MonotonePageResourceConditional {
    Contiguous {
        start: Address,
        /// Current frontier of zeroing, in a separate zeroing thread.
        zeroing_cursor: Address,
        /// Current limit of zeroing. If zeroingCursor < zeroingSentinel, zeroing is still happening.
        zeroing_sentinel: Address,
    },
    Discontiguous,
}

impl<VM: VMBinding> PageResource<VM> for MonotonePageResource<VM> {
    fn common(&self) -> &CommonPageResource {
        &self.common
    }
    fn common_mut(&mut self) -> &mut CommonPageResource {
        &mut self.common
    }

    fn reserve_pages(&self, pages: usize) -> usize {
        self.common().accounting.reserve(pages);
        pages
    }

    fn get_available_physical_pages(&self) -> usize {
        let sync = self.sync.lock().unwrap();
        let mut rtn = bytes_to_pages_up(sync.sentinel - sync.cursor);
        if !self.common.contiguous {
            rtn += self.common.vm_map.get_available_discontiguous_chunks() * PAGES_IN_CHUNK;
        }
        rtn
    }

    fn alloc_pages(
        &self,
        space_descriptor: SpaceDescriptor,
        reserved_pages: usize,
        required_pages: usize,
        tls: VMThread,
    ) -> Result<PRAllocResult, PRAllocFail> {
        debug!(
            "In MonotonePageResource, reserved_pages = {}, required_pages = {}",
            reserved_pages, required_pages
        );
        let mut new_chunk = false;
        let mut sync = self.sync.lock().unwrap();
        let mut rtn = sync.cursor;
        debug!(
            "cursor = {}, sentinel = {}, current_chunk = {}",
            sync.cursor, sync.sentinel, sync.current_chunk
        );

        if cfg!(debug_assertions) {
            if sync.current_chunk > sync.cursor
                || (chunk_align_down(sync.cursor) != sync.current_chunk
                    && chunk_align_down(sync.cursor) != sync.current_chunk + BYTES_IN_CHUNK)
            {
                self.log_chunk_fields(
                    &sync,
                    space_descriptor,
                    "MonotonePageResource.alloc_pages:fail",
                );
            }
            assert!(sync.current_chunk <= sync.cursor);
            assert!(
                sync.cursor.is_zero()
                    || chunk_align_down(sync.cursor) == sync.current_chunk
                    || chunk_align_down(sync.cursor) == (sync.current_chunk + BYTES_IN_CHUNK)
            );
        }

        let bytes = pages_to_bytes(required_pages);
        let mut tmp = sync.cursor + bytes;

        if !self.common().contiguous && tmp > sync.sentinel {
            /* we're out of virtual memory within our discontiguous region, so ask for more */
            let required_chunks = required_chunks(required_pages);
            sync.current_chunk =
                self.common
                    .grow_discontiguous_space(space_descriptor, required_chunks, None);
            sync.cursor = sync.current_chunk;
            sync.sentinel = sync.cursor
                + if sync.current_chunk.is_zero() {
                    0
                } else {
                    required_chunks << LOG_BYTES_IN_CHUNK
                };
            rtn = sync.cursor;
            tmp = sync.cursor + bytes;
            new_chunk = true;
        }

        debug_assert!(rtn >= sync.cursor && rtn < sync.cursor + bytes);
        if tmp > sync.sentinel {
            Result::Err(PRAllocFail)
        } else {
            sync.cursor = tmp;
            debug!("update cursor = {}", tmp);

            /* In a contiguous space we can bump along into the next chunk, so preserve the currentChunk invariant */
            if self.common().contiguous && chunk_align_down(sync.cursor) != sync.current_chunk {
                debug_assert!(
                    chunk_align_down(sync.cursor) > sync.current_chunk,
                    "Not monotonic.  chunk_align_down(sync.cursor): {}, sync.current_chunk: {}",
                    chunk_align_down(sync.cursor),
                    sync.current_chunk,
                );
                sync.current_chunk = chunk_align_down(sync.cursor);
            }
            self.commit_pages(reserved_pages, required_pages, tls);

            Result::Ok(PRAllocResult {
                start: rtn,
                pages: required_pages,
                new_chunk,
            })
        }
    }
}

impl<VM: VMBinding> MonotonePageResource<VM> {
    pub fn new_contiguous(start: Address, bytes: usize, vm_map: &'static dyn VMMap) -> Self {
        let sentinel = start + bytes;

        MonotonePageResource {
            common: CommonPageResource::new(true, cfg!(target_pointer_width = "64"), vm_map),
            sync: Mutex::new(MonotonePageResourceSync {
                cursor: start,
                current_chunk: chunk_align_down(start),
                sentinel,
                conditional: MonotonePageResourceConditional::Contiguous {
                    start,
                    zeroing_cursor: sentinel,
                    zeroing_sentinel: start,
                },
            }),
            _p: PhantomData,
        }
    }

    pub fn new_discontiguous(vm_map: &'static dyn VMMap) -> Self {
        MonotonePageResource {
            common: CommonPageResource::new(false, true, vm_map),
            sync: Mutex::new(MonotonePageResourceSync {
                cursor: Address::ZERO,
                current_chunk: Address::ZERO,
                sentinel: Address::ZERO,
                conditional: MonotonePageResourceConditional::Discontiguous,
            }),
            _p: PhantomData,
        }
    }

    /// Get highwater mark of current monotone space.
    pub fn cursor(&self) -> Address {
        self.sync.lock().unwrap().cursor
    }

    fn log_chunk_fields(
        &self,
        sync: &MonotonePageResourceSync,
        space_descriptor: SpaceDescriptor,
        site: &str,
    ) {
        debug!(
            "[{:?}]{}: cursor={}, current_chunk={}, delta={}",
            space_descriptor,
            site,
            sync.cursor,
            sync.current_chunk,
            sync.cursor - sync.current_chunk
        );
    }

    /// Reset this page resource, reclaiming all pages and discontiguous chunks back to the global pool.
    pub fn reset(&self) {
        let mut guard = self.sync.lock().unwrap();
        self.common().accounting.reset();
        self.release_pages(&mut guard);
    }

    pub fn get_current_chunk(&self) -> Address {
        self.sync.lock().unwrap().current_chunk
    }

    // top might be Address::ZERO
    pub fn reset_cursor(&self, mut top: Address) {
        let mut guard = self.sync.lock().unwrap();
        if self.common.contiguous {
            let space_start = match guard.conditional {
                MonotonePageResourceConditional::Contiguous { start, .. } => start,
                _ => unreachable!(),
            };
            if top.is_zero() {
                top = space_start;
            }
            let cursor = top.align_up(BYTES_IN_PAGE);
            let chunk = chunk_align_down(top);
            let pages = bytes_to_pages_up(top - space_start);
            self.common.accounting.reset();
            self.common.accounting.reserve_and_commit(pages);
            guard.current_chunk = chunk;
            guard.cursor = cursor;
        } else {
            let live_size = if let Some((chunk_start, chunk_end, live_bytes)) =
                self.common.truncate_discontiguous_regions_at(top)
            {
                if !chunk_start.is_zero() {
                    let effective_top = if top.is_zero() { chunk_start } else { top };
                    guard.current_chunk = chunk_start;
                    guard.sentinel = chunk_end;
                    guard.cursor = effective_top.align_up(BYTES_IN_PAGE);
                }
                live_bytes
            } else {
                0
            };
            let pages = bytes_to_pages_up(live_size);
            self.common.accounting.reset();
            self.common.accounting.reserve_and_commit(pages);
        }
    }

    fn release_pages(&self, guard: &mut MutexGuard<MonotonePageResourceSync>) {
        if self.common().contiguous {
            guard.cursor = match guard.conditional {
                MonotonePageResourceConditional::Contiguous { start, .. } => start,
                _ => unreachable!(),
            };
            guard.current_chunk = guard.cursor;
        } else if !guard.cursor.is_zero() {
            let bytes = guard.cursor - guard.current_chunk;
            debug_assert_eq!(bytes, pages_to_bytes(bytes_to_pages_up(bytes)));
            guard.current_chunk = Address::ZERO;
            guard.sentinel = Address::ZERO;
            guard.cursor = Address::ZERO;
            self.common.release_all_chunks();
        }
    }

    /// Iterate over all contiguous memory regions in this space.
    /// For contiguous space, this iterator yields a single region covering the allocated range.
    pub fn iterate_allocated_regions(&self) -> impl Iterator<Item = (Address, usize)> + '_ {
        let sync = self.sync.lock().unwrap();
        let cursor = sync.cursor;
        let regions: Vec<(Address, usize)> = match sync.conditional {
            MonotonePageResourceConditional::Contiguous { start, .. } => {
                let end = cursor.align_up(BYTES_IN_CHUNK);
                vec![(start, end - start)]
            }
            MonotonePageResourceConditional::Discontiguous => self
                .common
                .discontiguous_region_ranges()
                .into_iter()
                .map(|(start, region_size)| {
                    let size = if start < cursor && cursor < start + region_size {
                        cursor - start
                    } else {
                        region_size
                    };
                    (start, size)
                })
                .collect(),
        };
        regions.into_iter()
    }
}
