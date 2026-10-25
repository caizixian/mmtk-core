use super::{Block, BlockState};
use crate::util::alloc::allocator;
use crate::util::linear_scan::Region;
use crate::vm::VMBinding;
use std::marker::PhantomData;
use std::sync::atomic::AtomicBool;

/// List of blocks owned by the allocator.
///
/// # Representation Invariant
/// 1. Either `self.first.is_none() && self.last.is_none()` (empty list), or
///    `self.first == Some(head) && self.last == Some(tail)` forming a finite, acyclic
///    doubly-linked list of distinct blocks via `Block::PREV_BLOCK_TABLE` and
///    `Block::NEXT_BLOCK_TABLE` (`head` has `prev == None`, `tail` has `next == None`, and every
///    adjacent pair `(a, b)` satisfies `a.next == Some(b)` and `b.prev == Some(a)`).
/// 2. Every `Block` in the list belongs to an allocated `MarkSweepSpace` chunk with mapped local
///    side metadata (`PREV_BLOCK_TABLE`, `NEXT_BLOCK_TABLE`, `FREE_LIST_TABLE`, `SIZE_TABLE`,
///    `MARK_TABLE`, `TLS_TABLE`, `BLOCK_LIST_TABLE`).
/// 3. Exclusive ownership: each `Block` belongs to at most one `BlockList` at a time, and while it
///    remains in `self`, its `PREV_BLOCK_TABLE` and `NEXT_BLOCK_TABLE` links are mutated only via
///    `&mut self` (with no concurrent writes to `FREE_LIST_TABLE` while `&self` or `&mut self`
///    methods inspect list blocks).
/// 4. `self.size` is an immutable, non-zero, word-aligned MiMalloc bin size (`<= Block::BYTES`)
///    established by [`new_empty_block_lists`]. All fields are module-private to enforce list and
///    cell-size invariants while preserving the `#[repr(C)]` layout embedded in
///    [`crate::util::alloc::FreeListAllocator`].
#[repr(C)]
pub struct BlockList {
    first: Option<Block>,
    last: Option<Block>,
    size: usize,
    // Retained to preserve the `#[repr(C)]` layout of `BlockList`.
    _lock: AtomicBool,
}

impl std::fmt::Debug for BlockList {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "BlockList {:?}", self.iter().collect::<Vec<Block>>())
    }
}

impl BlockList {
    const fn new(size: usize) -> BlockList {
        BlockList {
            first: None,
            last: None,
            size,
            _lock: AtomicBool::new(false),
        }
    }

    /// Return the cell size (in bytes) for blocks in this bin.
    pub(crate) const fn size(&self) -> usize {
        self.size
    }

    /// List has no blocks
    pub fn is_empty(&self) -> bool {
        self.first.is_none()
    }

    /// Return the first block in the list without removing it.
    pub fn first(&self) -> Option<Block> {
        self.first
    }

    /// Remove `block` from `self`.
    ///
    /// # Safety
    /// `block` must currently be a member of `self`.
    unsafe fn remove(&mut self, block: Block) {
        // SAFETY: Caller guarantees `block` is in `self`; by `BlockList`'s invariant, `block` has
        // mapped `PREV_BLOCK_TABLE` and `NEXT_BLOCK_TABLE` metadata exclusively owned by `self`.
        let prev_next = unsafe { (block.load_prev_block(), block.load_next_block()) };
        match prev_next {
            (None, None) => {
                debug_assert_eq!(self.first, Some(block));
                debug_assert_eq!(self.last, Some(block));
                self.first = None;
                self.last = None;
            }
            (None, Some(next)) => {
                debug_assert_eq!(self.first, Some(block));
                // SAFETY: `next` is in `self` with mapped `PREV_BLOCK_TABLE` exclusively owned via `&mut self`.
                unsafe { next.clear_prev_block() };
                self.first = Some(next);
            }
            (Some(prev), None) => {
                debug_assert_eq!(self.last, Some(block));
                // SAFETY: `prev` is in `self` with mapped `NEXT_BLOCK_TABLE` exclusively owned via `&mut self`.
                unsafe { prev.clear_next_block() };
                self.last = Some(prev);
            }
            (Some(prev), Some(next)) => {
                // SAFETY: Both `prev` and `next` are distinct members of `self` with mapped link
                // side metadata exclusively owned via `&mut self`.
                unsafe {
                    prev.store_next_block(next);
                    next.store_prev_block(prev);
                }
            }
        }
        // SAFETY: `block` has mapped link metadata and is exclusively owned here as it leaves `self`.
        unsafe {
            block.clear_prev_block();
            block.clear_next_block();
        }
    }

    /// Pop the first block in the list
    pub fn pop(&mut self) -> Option<Block> {
        if let Some(head) = self.first {
            // SAFETY: `head` is in `self`, so by `BlockList`'s invariant its `NEXT_BLOCK_TABLE`
            // metadata is mapped and exclusively owned via `&mut self`.
            if let Some(next) = unsafe { head.load_next_block() } {
                self.first = Some(next);
                // SAFETY: `next` is in `self` with mapped `PREV_BLOCK_TABLE` exclusively owned via `&mut self`.
                unsafe { next.clear_prev_block() };
            } else {
                self.first = None;
                self.last = None;
            }
            // SAFETY: `head` was in `self` with mapped link metadata exclusively owned via `&mut self`.
            unsafe {
                head.clear_next_block();
                head.clear_prev_block();
            }
            Some(head)
        } else {
            None
        }
    }

    /// Push `block` to the front of the list.
    ///
    /// # Safety
    /// - `block` must belong to an allocated `MarkSweepSpace` chunk with mapped local side metadata
    ///   (`PREV_BLOCK_TABLE`, `NEXT_BLOCK_TABLE`, `FREE_LIST_TABLE`, `SIZE_TABLE`, `MARK_TABLE`,
    ///   `TLS_TABLE`, `BLOCK_LIST_TABLE`).
    /// - `block` must not currently belong to `self` or any other `BlockList`, and the caller
    ///   transfers exclusive ownership of `block`'s list-link side metadata (`PREV_BLOCK_TABLE`,
    ///   `NEXT_BLOCK_TABLE`) to `self` while `block` remains in `self` (with no concurrent writes to
    ///   `FREE_LIST_TABLE` while `&self` or `&mut self` methods inspect list blocks).
    pub unsafe fn push(&mut self, block: Block) {
        if self.is_empty() {
            // SAFETY: Caller guarantees `block` has mapped link metadata and is exclusively owned.
            unsafe {
                block.clear_next_block();
                block.clear_prev_block();
            }
            self.first = Some(block);
            self.last = Some(block);
        } else {
            let self_head = self.first.unwrap();
            // SAFETY: Caller guarantees `block` is distinct from `self_head` with mapped link
            // metadata, and `self_head` is in `self` with mapped link metadata owned via `&mut self`.
            unsafe {
                block.store_next_block(self_head);
                self_head.store_prev_block(block);
                block.clear_prev_block();
            }
            self.first = Some(block);
        }
    }

    /// Moves all the blocks of `other` into `self`, leaving `other` empty.
    pub fn append(&mut self, other: &mut BlockList) {
        debug_assert_eq!(self.size, other.size);
        if !other.is_empty() {
            debug_assert!(
                // SAFETY: `other.first` is in `other` with mapped `PREV_BLOCK_TABLE` metadata.
                unsafe { other.first.unwrap().load_prev_block() }.is_none(),
                "The other list's head has prev block"
            );
            if self.is_empty() {
                self.first = other.first;
                self.last = other.last;
            } else {
                debug_assert!(
                    // SAFETY: `self.first` is in `self` with mapped `PREV_BLOCK_TABLE` metadata.
                    unsafe { self.first.unwrap().load_prev_block() }.is_none(),
                    "Current list's head has prev block"
                );
                let self_tail = self.last.unwrap();
                let other_head = other.first.unwrap();
                // SAFETY: `self` and `other` are disjoint `&mut BlockList`s, so by `BlockList`'s
                // invariant their block sets are disjoint and both `self_tail` and `other_head`
                // have mapped link side metadata exclusively owned here.
                unsafe {
                    self_tail.store_next_block(other_head);
                    other_head.store_prev_block(self_tail);
                }
                self.last = other.last;
            }
            other.reset();
        }
    }

    /// Remove all blocks
    fn reset(&mut self) {
        self.first = None;
        self.last = None;
    }

    /// Move all blocks in `self` that currently have free cells to `dest`.
    pub(crate) fn transfer_available_blocks(&mut self, dest: &mut BlockList) {
        debug_assert_eq!(self.size, dest.size);
        let mut cursor = self.first;
        while let Some(block) = cursor {
            // SAFETY: `block` is currently in `self`; by `BlockList`'s invariant its
            // `NEXT_BLOCK_TABLE` and `FREE_LIST_TABLE` metadata are mapped with no concurrent writes.
            let next = unsafe { block.load_next_block() };
            // SAFETY: Same as above (`FREE_LIST_TABLE` is mapped with no concurrent writes).
            let has_free = unsafe { block.has_free_cells() };
            if has_free {
                // SAFETY: `block` is currently a member of `self`.
                unsafe { self.remove(block) };
                // SAFETY: `block` was just removed from `self` (which is disjoint from `dest`),
                // so it belongs to no `BlockList` and preserves `BlockList`'s invariant on `dest`.
                unsafe { dest.push(block) };
            }
            cursor = next;
        }
    }

    /// Get an iterator for the block list.
    pub fn iter(&self) -> BlockListIterator<'_> {
        BlockListIterator {
            cursor: self.first,
            _list: PhantomData,
        }
    }

    /// Release unmarked blocks, and sweep other blocks in the block list. Used by eager sweeping.
    ///
    /// # Safety
    /// Every block in `self` must belong to `space`, and no mutator or other GC worker may
    /// concurrently access blocks in `self`, their side metadata, or their unmarked cells.
    pub unsafe fn release_and_sweep_blocks<VM: VMBinding>(
        &mut self,
        space: &super::MarkSweepSpace<VM>,
    ) {
        let mut cursor = self.first;
        while let Some(block) = cursor {
            // SAFETY: `block` is currently in `self` with mapped `NEXT_BLOCK_TABLE` owned via `&mut self`.
            let next = unsafe { block.load_next_block() };
            match block.get_state() {
                BlockState::Unallocated => unreachable!(),
                BlockState::Unmarked => {
                    // SAFETY: `block` is currently a member of `self`.
                    unsafe { self.remove(block) };
                    // SAFETY: `block` has been removed from `self`, and caller guarantees it
                    // belongs to `space` with no concurrent access.
                    unsafe { space.release_block(block) };
                }
                BlockState::Marked => {
                    // SAFETY: Caller guarantees `block` belongs to `space` with mapped metadata/cells
                    // and exclusive access to its free list and unmarked cells.
                    unsafe { block.sweep::<VM>() };
                }
            }
            cursor = next;
        }
    }

    /// Release unmarked blocks, and do not sweep any blocks. Used by lazy sweeping
    ///
    /// # Safety
    /// Every block in `self` must belong to `space`, and no mutator or other GC worker may
    /// concurrently access blocks in `self` or their side metadata.
    pub unsafe fn release_blocks<VM: VMBinding>(&mut self, space: &super::MarkSweepSpace<VM>) {
        let mut cursor = self.first;
        while let Some(block) = cursor {
            // SAFETY: `block` is currently in `self` with mapped `NEXT_BLOCK_TABLE` owned via `&mut self`.
            let next = unsafe { block.load_next_block() };
            match block.get_state() {
                BlockState::Unallocated => unreachable!(),
                BlockState::Unmarked => {
                    // SAFETY: `block` is currently a member of `self`.
                    unsafe { self.remove(block) };
                    // SAFETY: `block` has been removed from `self`, and caller guarantees it
                    // belongs to `space` with no concurrent access.
                    unsafe { space.release_block(block) };
                }
                BlockState::Marked => {}
            }
            cursor = next;
        }
    }
}

pub struct BlockListIterator<'a> {
    cursor: Option<Block>,
    _list: PhantomData<&'a BlockList>,
}

impl Iterator for BlockListIterator<'_> {
    type Item = Block;

    fn next(&mut self) -> Option<Self::Item> {
        let ret = self.cursor;
        if let Some(cur) = self.cursor {
            // SAFETY: `self._list` borrows the owning `&'a BlockList` for the lifetime of this
            // iterator, preventing concurrent mutation, and `cur` is a member of that `BlockList`
            // with mapped `NEXT_BLOCK_TABLE` metadata by `BlockList`'s invariant.
            self.cursor = unsafe { cur.load_next_block() };
        }
        ret
    }
}

/// Log2 of pointer size
const MI_INTPTR_SHIFT: usize = crate::util::constants::LOG_BYTES_IN_ADDRESS as usize;
/// pointer size in bytes
const MI_INTPTR_SIZE: usize = 1 << MI_INTPTR_SHIFT;
/// pointer size in bits
const MI_INTPTR_BITS: usize = MI_INTPTR_SIZE * 8;
/// Number of bins in BlockLists. Reserve bin0 as an empty bin.
pub(crate) const MI_BIN_FULL: usize = MAX_BIN + 1;
/// The largest valid bin.
pub(crate) const MAX_BIN: usize = 48;

/// Largest object size allowed with our mimalloc implementation, in bytes
pub(crate) const MI_LARGE_OBJ_SIZE_MAX: usize =
    crate::util::rust_util::min_of_usize(Block::BYTES, MAX_BIN_SIZE);
/// Largest object size in words
const MI_LARGE_OBJ_WSIZE_MAX: usize = MI_LARGE_OBJ_SIZE_MAX / MI_INTPTR_SIZE;
/// The object size for the last bin. We should not try allocate objects larger than this with the allocator.
pub(crate) const MAX_BIN_SIZE: usize = 8192 * MI_INTPTR_SIZE;

/// All the bins for the block lists
// Each block list takes roughly 8bytes * 4 * 49 = 1658 bytes. It is more reasonable to heap allocate them, and
// just put them behind a boxed pointer.
pub type BlockLists = Box<[BlockList; MAX_BIN + 1]>;

/// Create an empty set of block lists of different size classes (bins)
pub(crate) fn new_empty_block_lists() -> BlockLists {
    let ret = Box::new([
        BlockList::new(MI_INTPTR_SIZE),
        BlockList::new(MI_INTPTR_SIZE),
        BlockList::new(2 * MI_INTPTR_SIZE),
        BlockList::new(3 * MI_INTPTR_SIZE),
        BlockList::new(4 * MI_INTPTR_SIZE),
        BlockList::new(5 * MI_INTPTR_SIZE),
        BlockList::new(6 * MI_INTPTR_SIZE),
        BlockList::new(7 * MI_INTPTR_SIZE),
        BlockList::new(8 * MI_INTPTR_SIZE), /* 8 */
        BlockList::new(10 * MI_INTPTR_SIZE),
        BlockList::new(12 * MI_INTPTR_SIZE),
        BlockList::new(14 * MI_INTPTR_SIZE),
        BlockList::new(16 * MI_INTPTR_SIZE),
        BlockList::new(20 * MI_INTPTR_SIZE),
        BlockList::new(24 * MI_INTPTR_SIZE),
        BlockList::new(28 * MI_INTPTR_SIZE),
        BlockList::new(32 * MI_INTPTR_SIZE), /* 16 */
        BlockList::new(40 * MI_INTPTR_SIZE),
        BlockList::new(48 * MI_INTPTR_SIZE),
        BlockList::new(56 * MI_INTPTR_SIZE),
        BlockList::new(64 * MI_INTPTR_SIZE),
        BlockList::new(80 * MI_INTPTR_SIZE),
        BlockList::new(96 * MI_INTPTR_SIZE),
        BlockList::new(112 * MI_INTPTR_SIZE),
        BlockList::new(128 * MI_INTPTR_SIZE), /* 24 */
        BlockList::new(160 * MI_INTPTR_SIZE),
        BlockList::new(192 * MI_INTPTR_SIZE),
        BlockList::new(224 * MI_INTPTR_SIZE),
        BlockList::new(256 * MI_INTPTR_SIZE),
        BlockList::new(320 * MI_INTPTR_SIZE),
        BlockList::new(384 * MI_INTPTR_SIZE),
        BlockList::new(448 * MI_INTPTR_SIZE),
        BlockList::new(512 * MI_INTPTR_SIZE), /* 32 */
        BlockList::new(640 * MI_INTPTR_SIZE),
        BlockList::new(768 * MI_INTPTR_SIZE),
        BlockList::new(896 * MI_INTPTR_SIZE),
        BlockList::new(1024 * MI_INTPTR_SIZE),
        BlockList::new(1280 * MI_INTPTR_SIZE),
        BlockList::new(1536 * MI_INTPTR_SIZE),
        BlockList::new(1792 * MI_INTPTR_SIZE),
        BlockList::new(2048 * MI_INTPTR_SIZE), /* 40 */
        BlockList::new(2560 * MI_INTPTR_SIZE),
        BlockList::new(3072 * MI_INTPTR_SIZE),
        BlockList::new(3584 * MI_INTPTR_SIZE),
        BlockList::new(4096 * MI_INTPTR_SIZE),
        BlockList::new(5120 * MI_INTPTR_SIZE),
        BlockList::new(6144 * MI_INTPTR_SIZE),
        BlockList::new(7168 * MI_INTPTR_SIZE),
        BlockList::new(8192 * MI_INTPTR_SIZE), /* 48 */
    ]);

    debug_assert_eq!(
        ret[MAX_BIN].size, MAX_BIN_SIZE,
        "MAX_BIN_SIZE = {}, actual max bin size  = {}, please update the constants",
        MAX_BIN_SIZE, ret[MAX_BIN].size
    );

    ret
}

/// Returns how many pages the block lists uses.
#[allow(unused)]
pub(crate) fn pages_used_by_blocklists(lists: &BlockLists) -> usize {
    let mut pages = 0;
    for bin in 1..=MAX_BIN {
        for _block in lists[bin].iter() {
            pages += Block::BYTES >> crate::util::constants::LOG_BYTES_IN_PAGE;
        }
    }

    pages
}

/// Align a byte size to a size in machine words
/// i.e. byte size == `wsize*sizeof(void*)`
/// adapted from _mi_wsize_from_size in mimalloc
fn mi_wsize_from_size(size: usize) -> usize {
    size.div_ceil(MI_INTPTR_SIZE)
}

pub fn mi_bin<VM: VMBinding>(size: usize, align: usize) -> usize {
    let size = allocator::get_maximum_aligned_size::<VM>(size, align);
    mi_bin_from_size(size)
}

fn mi_bin_from_size(size: usize) -> usize {
    // adapted from _mi_bin in mimalloc
    let mut wsize: usize = mi_wsize_from_size(size);
    debug_assert!(wsize <= MI_LARGE_OBJ_WSIZE_MAX);
    if wsize <= 1 {
        1
    } else if wsize <= 8 {
        wsize
        // bin = ((wsize + 1) & !1) as u8; // round to double word sizes
    } else {
        wsize -= 1;
        let b = (MI_INTPTR_BITS - 1 - usize::leading_zeros(wsize) as usize) as u8; // note: wsize != 0
        (((b << 2) + ((wsize >> (b - 2)) & 0x03) as u8) - 3) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get_bin_size_range(bin: usize, bins: &BlockLists) -> Option<(usize, usize)> {
        if bin == 0 || bin > MAX_BIN {
            None
        } else if bin == 1 {
            Some((0, bins[1].size))
        } else {
            Some((bins[bin - 1].size, bins[bin].size))
        }
    }

    #[test]
    fn test_mi_bin() {
        let block_lists = new_empty_block_lists();
        for size in 0..=MAX_BIN_SIZE {
            let bin = mi_bin_from_size(size);
            let bin_range = get_bin_size_range(bin, &block_lists);
            assert!(bin_range.is_some(), "Invalid bin {} for size {}", bin, size);
            assert!(
                size >= bin_range.unwrap().0 && bin < bin_range.unwrap().1,
                "Assigning size={} to bin={} ({:?}) incorrect",
                size,
                bin,
                bin_range.unwrap()
            );
        }
    }
}
