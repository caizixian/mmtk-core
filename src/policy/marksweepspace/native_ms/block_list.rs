#![deny(unsafe_op_in_unsafe_fn)]

use super::{Block, BlockState, IntrusiveBlock, SweptBlock, UnsweptBlock};
use crate::util::alloc::allocator;
use crate::util::constants::MIN_OBJECT_SIZE;
use crate::util::linear_scan::Region;
use crate::util::rust_util::fin::Fin;
use crate::util::Address;
use crate::vm::VMBinding;
use std::marker::PhantomData;
use std::ops::Deref;

/// Intrusive doubly-linked list of blocks of homogeneous linear typestate `B`
/// ([`SweptBlock`] or [`UnsweptBlock`]) belonging to a single MiMalloc size class.
///
/// # `#[repr(C)]` Memory Layout
/// `B` is sealed to [`SweptBlock`] and [`UnsweptBlock`], both `#[repr(transparent)]` over
/// [`Block`]`(`[`crate::util::AlignedAddress`]`<16>)` (`NonNull<u8>`), so `Option<B>` uses Rust's
/// guaranteed null-pointer optimization (`0` = `None`, non-zero 64 KiB-aligned address = `Some`)
/// and is pointer-sized and `#[repr(C)]`-compatible:
///
/// | Field | Type | 64-bit Offset | 64-bit Size | 32-bit Offset | 32-bit Size |
/// | :--- | :--- | :---: | :---: | :---: | :---: |
/// | `first` | `Option<B>` | `0` | `8 B` | `0` | `4 B` |
/// | `last` | `Option<B>` | `8` | `8 B` | `4` | `4 B` |
/// | `size` | `usize` | `16` | `8 B` | `8` | `4 B` |
/// | **Total** | `BlockList<B>` | — | **`24 B` (align `8 B`)** | — | **`12 B` (align `4 B`)** |
///
/// *(Migration note: removing the unused `_lock: AtomicBool` field shrank `BlockList` from `32 B`
/// to `24 B` on 64-bit (`16 B -> 12 B` on 32-bit), saving `392 B` per 49-bin array and `1,568 B`
/// per [`crate::util::alloc::FreeListAllocator`] on 64-bit.)*
///
/// # Representation Invariant
/// 1. Either `self.first.is_none() && self.last.is_none()` (empty list), or
///    `self.first == Some(head) && self.last == Some(tail)` forming a finite, acyclic
///    doubly-linked list of distinct blocks via `Block::PREV_BLOCK_TABLE` and
///    `Block::NEXT_BLOCK_TABLE` (`head` has `prev == None`, `tail` has `next == None`, and every
///    adjacent pair `(a, b)` satisfies `a.next == Some(b)` and `b.prev == Some(a)`).
/// 2. Every block in the list upholds `B`'s spatial, side-metadata, cell-size, and (for
///    [`SweptBlock`]) intrusive free-cell chain invariants (with list-link exclusivity held by
///    `self` rather than a detached `B` value).
/// 3. Exclusive ownership: each block belongs to at most one `BlockList` (and no detached `B`
///    token) at a time; while in `self`, its `PREV_BLOCK_TABLE` and `NEXT_BLOCK_TABLE` links are
///    mutated only via `&mut self`.
/// 4. `self.size` is an immutable, non-zero, word-aligned MiMalloc bin size (`<= Block::BYTES`),
///    and every block pushed or appended into `self` satisfies `block.cell_size() == self.size`.
#[repr(C)]
pub struct BlockList<B: IntrusiveBlock = SweptBlock> {
    first: Option<B>,
    last: Option<B>,
    size: usize,
}

static_assertions::assert_eq_size!(BlockList<SweptBlock>, [usize; 3]);
static_assertions::assert_eq_align!(BlockList<SweptBlock>, usize);
static_assertions::assert_eq_size!(BlockList<UnsweptBlock>, [usize; 3]);
static_assertions::assert_eq_align!(BlockList<UnsweptBlock>, usize);

impl<B: IntrusiveBlock> std::fmt::Debug for BlockList<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "BlockList {:?}", self.iter().collect::<Vec<Block>>())
    }
}

impl<B: IntrusiveBlock> BlockList<B> {
    const fn new(size: usize) -> Self {
        Self {
            first: None,
            last: None,
            size,
        }
    }

    /// Return the cell size (in bytes) for blocks in this bin.
    #[inline(always)]
    pub(crate) const fn size(&self) -> usize {
        self.size
    }

    /// Return `true` if the list contains no blocks.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.first.is_none()
    }

    /// Return the [`Block`] handle of the front block without unlinking it.
    #[inline(always)]
    pub fn first(&self) -> Option<Block> {
        self.first.as_ref().map(IntrusiveBlock::block)
    }

    /// Unlink `block` from `self`, clearing its `PREV_BLOCK_TABLE` and `NEXT_BLOCK_TABLE` links
    /// and returning detached linear ownership `B`.
    ///
    /// # Safety
    /// `block` must currently be a member of `self`.
    unsafe fn remove_raw(&mut self, block: Block) -> B {
        // SAFETY: Caller guarantees `block` is in `self`; by `BlockList`'s invariant, `block` has
        // mapped `PREV_BLOCK_TABLE` and `NEXT_BLOCK_TABLE` metadata exclusively owned by `self`.
        let prev_next = unsafe { (block.load_prev_block(), block.load_next_block()) };
        match prev_next {
            (None, None) => {
                debug_assert_eq!(self.first(), Some(block));
                debug_assert_eq!(self.last.as_ref().map(IntrusiveBlock::block), Some(block));
                self.first = None;
                self.last = None;
            }
            (None, Some(next)) => {
                debug_assert_eq!(self.first(), Some(block));
                // SAFETY: `next` is in `self` with mapped `PREV_BLOCK_TABLE` owned via `&mut self`,
                // and becomes the new head endpoint of `self`.
                self.first = Some(unsafe {
                    next.clear_prev_block();
                    B::from_block_unchecked(next)
                });
            }
            (Some(prev), None) => {
                debug_assert_eq!(self.last.as_ref().map(IntrusiveBlock::block), Some(block));
                // SAFETY: `prev` is in `self` with mapped `NEXT_BLOCK_TABLE` owned via `&mut self`,
                // and becomes the new tail endpoint of `self`.
                self.last = Some(unsafe {
                    prev.clear_next_block();
                    B::from_block_unchecked(prev)
                });
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
        // SAFETY: `block` has mapped link metadata and is now detached from `self` with `prev == None`
        // and `next == None` and unique ownership transferred to the returned `B`.
        unsafe {
            block.clear_prev_block();
            block.clear_next_block();
            B::from_block_unchecked(block)
        }
    }

    /// Pop the front block from the list, returning detached linear ownership `Option<B>`.
    pub fn pop(&mut self) -> Option<B> {
        let head = self.first.take()?;
        let raw_head = head.block();
        // SAFETY: `head` was in `self`, so its `NEXT_BLOCK_TABLE` and `PREV_BLOCK_TABLE` metadata
        // are mapped and exclusively owned via `&mut self`.
        unsafe {
            if let Some(next) = raw_head.load_next_block() {
                next.clear_prev_block();
                self.first = Some(B::from_block_unchecked(next));
            } else {
                self.last = None;
            }
            raw_head.clear_next_block();
            raw_head.clear_prev_block();
        }
        Some(head)
    }

    /// Push a detached owned `block` to the front of `self`.
    ///
    /// Panics if `block.cell_size() != self.size`.
    pub fn push(&mut self, block: B) {
        let block_cell_size = block.cell_size();
        assert_eq!(
            block_cell_size, self.size,
            "BlockList::push cell size mismatch: block has {} B cells, bin expects {} B",
            block_cell_size, self.size,
        );
        let raw = block.block();
        if self.is_empty() {
            // SAFETY: `block` is a detached `B` with mapped link metadata owned by value here;
            // `self.first` and `self.last` record the single-element list endpoints inside `self`.
            unsafe {
                raw.clear_next_block();
                raw.clear_prev_block();
                self.first = Some(B::from_block_unchecked(raw));
            }
            self.last = Some(block);
        } else {
            let self_head = self.first.as_ref().unwrap().block();
            // SAFETY: `block` is a detached `B` (hence distinct from `self_head` in `self`), and
            // both `raw` and `self_head` have mapped link metadata owned via `block` and `&mut self`.
            unsafe {
                raw.store_next_block(self_head);
                self_head.store_prev_block(raw);
                raw.clear_prev_block();
            }
            self.first = Some(block);
        }
    }

    /// Move all blocks of `other` onto the tail of `self` in `O(1)`, leaving `other` empty.
    ///
    /// Panics if `self.size != other.size`.
    pub fn append(&mut self, other: &mut BlockList<B>) {
        assert_eq!(
            self.size, other.size,
            "BlockList::append bin cell size mismatch: {} != {}",
            self.size, other.size,
        );
        if let (Some(other_first), Some(other_last)) = (other.first.take(), other.last.take()) {
            let other_head = other_first.block();
            debug_assert!(
                // SAFETY: `other_head` was head of `other` with mapped `PREV_BLOCK_TABLE` metadata.
                unsafe { other_head.load_prev_block() }.is_none(),
                "The other list's head has prev block"
            );
            if self.is_empty() {
                self.first = Some(other_first);
                self.last = Some(other_last);
            } else {
                debug_assert!(
                    // SAFETY: `self.first` is in `self` with mapped `PREV_BLOCK_TABLE` metadata.
                    unsafe { self.first.as_ref().unwrap().block().load_prev_block() }.is_none(),
                    "Current list's head has prev block"
                );
                let self_tail = self.last.as_ref().unwrap().block();
                // SAFETY: `self` and `other` are disjoint `&mut BlockList<B>`s, so their block
                // chains are disjoint and both `self_tail` and `other_head` have mapped link side
                // metadata exclusively owned here.
                unsafe {
                    self_tail.store_next_block(other_head);
                    other_head.store_prev_block(self_tail);
                }
                self.last = Some(other_last);
            }
        }
    }

    /// Return a read-only iterator over the [`Block`] handles in `self`.
    pub fn iter(&self) -> BlockListIterator<'_, B> {
        BlockListIterator {
            cursor: self.first(),
            _list: PhantomData,
        }
    }

    /// Unlink and release all `BlockState::Unmarked` blocks in `self` back to `space`, leaving
    /// `BlockState::Marked` blocks in `self`.
    pub fn release_blocks<VM: VMBinding>(&mut self, space: &super::MarkSweepSpace<VM>) {
        let mut cursor = self.first();
        while let Some(block) = cursor {
            // SAFETY: `block` is currently in `self` with mapped `NEXT_BLOCK_TABLE` owned via `&mut self`.
            let next = unsafe { block.load_next_block() };
            match block.get_state() {
                BlockState::Unallocated => unreachable!("Unallocated block in BlockList"),
                BlockState::Unmarked => {
                    // SAFETY: `block` is currently a member of `self`.
                    let detached = unsafe { self.remove_raw(block) };
                    space.release_block(detached);
                }
                BlockState::Marked => {}
            }
            cursor = next;
        }
    }
}

impl BlockList<SweptBlock> {
    /// Return `true` iff `self` is non-empty and its front [`SweptBlock`] has at least one free cell.
    #[inline]
    pub(crate) fn has_head_free_cells(&self) -> bool {
        self.first.as_ref().is_some_and(SweptBlock::has_free_cells)
    }

    /// Pop and zero one cell from the front [`SweptBlock`] of `self` without unlinking the block,
    /// returning `Address::ZERO` if `self` is empty or its front block has no free cells.
    #[inline]
    pub(crate) fn alloc_cell_from_head(&mut self) -> Address {
        match self.first.as_mut() {
            Some(head) => head.alloc_cell(),
            None => Address::ZERO,
        }
    }

    /// Move all blocks in `self` that currently have free cells to `dest`.
    pub(crate) fn transfer_available_blocks(&mut self, dest: &mut BlockList<SweptBlock>) {
        assert_eq!(
            self.size, dest.size,
            "BlockList::transfer_available_blocks bin cell size mismatch: {} != {}",
            self.size, dest.size,
        );
        let mut cursor = self.first();
        while let Some(block) = cursor {
            // SAFETY: `block` is currently in `self`; by `BlockList<SweptBlock>`'s invariant its
            // `NEXT_BLOCK_TABLE` and `FREE_LIST_TABLE` metadata are mapped with no concurrent writes.
            let (next, has_free) = unsafe { (block.load_next_block(), block.has_free_cells()) };
            if has_free {
                // SAFETY: `block` is currently a member of `self`.
                let detached = unsafe { self.remove_raw(block) };
                dest.push(detached);
            }
            cursor = next;
        }
    }

    /// Release all `BlockState::Unmarked` blocks in `self` to `space` and move all remaining
    /// `BlockState::Marked` blocks onto `unswept` in `O(1)` for lazy sweeping.
    pub(crate) fn release_blocks_and_move_marked_to_unswept<VM: VMBinding>(
        &mut self,
        space: &super::MarkSweepSpace<VM>,
        unswept: &mut BlockList<UnsweptBlock>,
    ) {
        assert_eq!(
            self.size, unswept.size,
            "BlockList::release_blocks_and_move_marked_to_unswept bin cell size mismatch: {} != {}",
            self.size, unswept.size,
        );
        self.release_blocks(space);

        if let (Some(first_swept), Some(last_swept)) = (self.first.take(), self.last.take()) {
            let first_raw = first_swept.block();
            let last_raw = last_swept.block();
            // SAFETY: `self.release_blocks(space)` above removed every `Unmarked` block (and
            // panicked on `Unallocated`), so every block remaining in the chain `[first_raw..=last_raw]`
            // has `MARK_TABLE == BlockState::Marked`, mapped side metadata, valid `SIZE_TABLE == self.size`,
            // and exclusive ownership transferred linearly from `self` (now empty) to `unswept`.
            let (first_unswept, last_unswept) = unsafe {
                (
                    UnsweptBlock::from_block_unchecked(first_raw),
                    UnsweptBlock::from_block_unchecked(last_raw),
                )
            };
            if unswept.is_empty() {
                unswept.first = Some(first_unswept);
                unswept.last = Some(last_unswept);
            } else {
                let unswept_tail = unswept.last.as_ref().unwrap().block();
                // SAFETY: `self` and `unswept` held disjoint block chains, and both `unswept_tail`
                // and `first_raw` have mapped link side metadata exclusively owned here.
                unsafe {
                    unswept_tail.store_next_block(first_raw);
                    first_raw.store_prev_block(unswept_tail);
                }
                unswept.last = Some(last_unswept);
            }
        }
    }
}

pub struct BlockListIterator<'a, B: IntrusiveBlock = SweptBlock> {
    cursor: Option<Block>,
    _list: PhantomData<&'a BlockList<B>>,
}

impl<B: IntrusiveBlock> Iterator for BlockListIterator<'_, B> {
    type Item = Block;

    fn next(&mut self) -> Option<Self::Item> {
        let ret = self.cursor;
        if let Some(cur) = self.cursor {
            // SAFETY: `self._list` borrows `&'a BlockList<B>` for the lifetime of this iterator,
            // preventing concurrent mutation, and `cur` is a member of that `BlockList` with
            // mapped `NEXT_BLOCK_TABLE` metadata by `BlockList`'s invariant.
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
/// Number of bins in `BlockLists`. Reserve bin 0 as an unused sentinel bin.
pub(crate) const MI_BIN_FULL: usize = MAX_BIN + 1;
/// The largest valid bin.
pub(crate) const MAX_BIN: usize = 48;

/// Bounded ordinal index `Fin<49>` (`0 <= bin <= MAX_BIN`) into [`BlockLists`] / [`UnsweptBlockLists`].
pub(crate) type BinIndex = Fin<{ MAX_BIN + 1 }>;

/// Largest object size allowed with our mimalloc implementation, in bytes
pub(crate) const MI_LARGE_OBJ_SIZE_MAX: usize =
    crate::util::rust_util::min_of_usize(Block::BYTES, MAX_BIN_SIZE);
/// Largest object size in words
const MI_LARGE_OBJ_WSIZE_MAX: usize = MI_LARGE_OBJ_SIZE_MAX / MI_INTPTR_SIZE;
/// The object size for the last bin. We should not try to allocate objects larger than this with the allocator.
pub(crate) const MAX_BIN_SIZE: usize = 8192 * MI_INTPTR_SIZE;

/// Canonical MiMalloc cell size (in bytes) for each bin `0..=MAX_BIN`.
pub(crate) const BIN_CELL_SIZES: [usize; MAX_BIN + 1] = [
    MI_INTPTR_SIZE,
    MI_INTPTR_SIZE,
    2 * MI_INTPTR_SIZE,
    3 * MI_INTPTR_SIZE,
    4 * MI_INTPTR_SIZE,
    5 * MI_INTPTR_SIZE,
    6 * MI_INTPTR_SIZE,
    7 * MI_INTPTR_SIZE,
    8 * MI_INTPTR_SIZE, /* 8 */
    10 * MI_INTPTR_SIZE,
    12 * MI_INTPTR_SIZE,
    14 * MI_INTPTR_SIZE,
    16 * MI_INTPTR_SIZE,
    20 * MI_INTPTR_SIZE,
    24 * MI_INTPTR_SIZE,
    28 * MI_INTPTR_SIZE,
    32 * MI_INTPTR_SIZE, /* 16 */
    40 * MI_INTPTR_SIZE,
    48 * MI_INTPTR_SIZE,
    56 * MI_INTPTR_SIZE,
    64 * MI_INTPTR_SIZE,
    80 * MI_INTPTR_SIZE,
    96 * MI_INTPTR_SIZE,
    112 * MI_INTPTR_SIZE,
    128 * MI_INTPTR_SIZE, /* 24 */
    160 * MI_INTPTR_SIZE,
    192 * MI_INTPTR_SIZE,
    224 * MI_INTPTR_SIZE,
    256 * MI_INTPTR_SIZE,
    320 * MI_INTPTR_SIZE,
    384 * MI_INTPTR_SIZE,
    448 * MI_INTPTR_SIZE,
    512 * MI_INTPTR_SIZE, /* 32 */
    640 * MI_INTPTR_SIZE,
    768 * MI_INTPTR_SIZE,
    896 * MI_INTPTR_SIZE,
    1024 * MI_INTPTR_SIZE,
    1280 * MI_INTPTR_SIZE,
    1536 * MI_INTPTR_SIZE,
    1792 * MI_INTPTR_SIZE,
    2048 * MI_INTPTR_SIZE, /* 40 */
    2560 * MI_INTPTR_SIZE,
    3072 * MI_INTPTR_SIZE,
    3584 * MI_INTPTR_SIZE,
    4096 * MI_INTPTR_SIZE,
    5120 * MI_INTPTR_SIZE,
    6144 * MI_INTPTR_SIZE,
    7168 * MI_INTPTR_SIZE,
    8192 * MI_INTPTR_SIZE, /* 48 */
];

// Compile-time verification that every bin size satisfies `SweptBlock` / `UnsweptBlock` cell-size
// invariants (`MIN_OBJECT_SIZE <= cs <= Block::BYTES` and word-aligned) and `BIN_CELL_SIZES[48] == MAX_BIN_SIZE`.
const _: () = {
    assert!(BIN_CELL_SIZES[MAX_BIN] == MAX_BIN_SIZE);
    let mut i = 0;
    while i <= MAX_BIN {
        let cs = BIN_CELL_SIZES[i];
        assert!(cs >= MIN_OBJECT_SIZE);
        assert!(cs <= Block::BYTES);
        assert!(cs % MI_INTPTR_SIZE == 0);
        i += 1;
    }
};

/// Return the canonical cell size in bytes for `bin` (`0` bounds checks via `Fin<49>`).
#[inline(always)]
pub(crate) const fn bin_cell_size(bin: BinIndex) -> usize {
    BIN_CELL_SIZES[bin.get()]
}

/// Sealed 49-bin array of [`SweptBlock`] lists (`#[repr(transparent)]` over
/// `Box<[BlockList<SweptBlock>; MAX_BIN + 1]>`, `1,176 B` on heap on 64-bit).
///
/// Implements read-only [`Deref`] to `[BlockList<SweptBlock>; MAX_BIN + 1]` without `DerefMut` or
/// `IndexMut` so safe code can never swap bins (`bin 1 <-> bin 48`) or swap swept and unswept
/// lists (`Bug D`). All bin-mutating methods are `pub(crate)` and indexed by [`BinIndex`] (`Fin<49>`).
#[repr(transparent)]
#[derive(Debug)]
pub struct BlockLists(Box<[BlockList<SweptBlock>; MAX_BIN + 1]>);

static_assertions::assert_eq_size!(BlockLists, usize);
static_assertions::assert_eq_align!(BlockLists, usize);

impl Deref for BlockLists {
    type Target = [BlockList<SweptBlock>; MAX_BIN + 1];

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl BlockLists {
    /// Allocate a fresh boxed array of 49 empty [`SweptBlock`] bin lists.
    pub(crate) fn new() -> Self {
        Self(Box::new(std::array::from_fn(|i| {
            BlockList::new(BIN_CELL_SIZES[i])
        })))
    }

    /// Push `block` onto the front of `bin`.
    #[inline]
    pub(crate) fn push(&mut self, bin: BinIndex, block: SweptBlock) {
        self.0[bin].push(block);
    }

    /// Pop the front [`SweptBlock`] from `bin`.
    #[inline]
    pub(crate) fn pop(&mut self, bin: BinIndex) -> Option<SweptBlock> {
        self.0[bin].pop()
    }

    /// Pop and zero one cell from the front block of `bin` without unlinking the block.
    #[inline]
    pub(crate) fn alloc_cell_from_head(&mut self, bin: BinIndex) -> Address {
        self.0[bin].alloc_cell_from_head()
    }

    /// Move all blocks from `other[bin]` onto the tail of `self[bin]`.
    #[inline]
    pub(crate) fn append_bin_from(&mut self, bin: BinIndex, other: &mut Self) {
        self.0[bin].append(&mut other.0[bin]);
    }

    /// Move all blocks in `self[bin]` that currently have free cells to `dest[bin]`.
    #[inline]
    pub(crate) fn transfer_available_bin_to(&mut self, bin: BinIndex, dest: &mut Self) {
        self.0[bin].transfer_available_blocks(&mut dest.0[bin]);
    }

    /// Release all `BlockState::Unmarked` blocks in `self[bin]` back to `space`.
    #[inline]
    pub(crate) fn release_blocks<VM: VMBinding>(
        &mut self,
        bin: BinIndex,
        space: &super::MarkSweepSpace<VM>,
    ) {
        self.0[bin].release_blocks(space);
    }

    /// Release all `BlockState::Unmarked` blocks in `self[bin]` to `space` and move surviving
    /// `BlockState::Marked` blocks onto `unswept[bin]` for lazy sweeping.
    #[inline]
    pub(crate) fn release_blocks_and_move_marked_to_unswept<VM: VMBinding>(
        &mut self,
        bin: BinIndex,
        space: &super::MarkSweepSpace<VM>,
        unswept: &mut UnsweptBlockLists,
    ) {
        self.0[bin].release_blocks_and_move_marked_to_unswept(space, &mut unswept.0[bin]);
    }
}

/// Sealed 49-bin array of [`UnsweptBlock`] lists (`#[repr(transparent)]` over
/// `Box<[BlockList<UnsweptBlock>; MAX_BIN + 1]>`, `1,176 B` on heap on 64-bit).
///
/// Implements read-only [`Deref`] to `[BlockList<UnsweptBlock>; MAX_BIN + 1]` without `DerefMut`
/// or `IndexMut` so safe code can never swap bins or confuse unswept blocks with swept blocks.
#[repr(transparent)]
#[derive(Debug)]
pub struct UnsweptBlockLists(Box<[BlockList<UnsweptBlock>; MAX_BIN + 1]>);

static_assertions::assert_eq_size!(UnsweptBlockLists, usize);
static_assertions::assert_eq_align!(UnsweptBlockLists, usize);

impl Deref for UnsweptBlockLists {
    type Target = [BlockList<UnsweptBlock>; MAX_BIN + 1];

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl UnsweptBlockLists {
    /// Allocate a fresh boxed array of 49 empty [`UnsweptBlock`] bin lists.
    pub(crate) fn new() -> Self {
        Self(Box::new(std::array::from_fn(|i| {
            BlockList::new(BIN_CELL_SIZES[i])
        })))
    }

    /// Pop the front [`UnsweptBlock`] from `bin`.
    #[inline]
    pub(crate) fn pop(&mut self, bin: BinIndex) -> Option<UnsweptBlock> {
        self.0[bin].pop()
    }

    /// Move all blocks from `other[bin]` onto the tail of `self[bin]`.
    #[inline]
    pub(crate) fn append_bin_from(&mut self, bin: BinIndex, other: &mut Self) {
        self.0[bin].append(&mut other.0[bin]);
    }

    /// Release all `BlockState::Unmarked` blocks in `self[bin]` back to `space`.
    #[inline]
    pub(crate) fn release_blocks<VM: VMBinding>(
        &mut self,
        bin: BinIndex,
        space: &super::MarkSweepSpace<VM>,
    ) {
        self.0[bin].release_blocks(space);
    }
}

/// Create an empty set of [`SweptBlock`] lists of different size classes (bins).
pub(crate) fn new_empty_block_lists() -> BlockLists {
    BlockLists::new()
}

/// Create an empty set of [`UnsweptBlock`] lists of different size classes (bins).
pub(crate) fn new_empty_unswept_block_lists() -> UnsweptBlockLists {
    UnsweptBlockLists::new()
}

/// Returns how many OS pages the block lists use.
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

/// Compute the bounded [`BinIndex`] (`Fin<49>`, `1 <= bin <= MAX_BIN`) for `(size, align)`,
/// panicking if the maximum aligned allocation size exceeds [`MAX_BIN_SIZE`].
#[inline]
pub(crate) fn mi_bin_bounded<VM: VMBinding>(size: usize, align: usize) -> BinIndex {
    let aligned_size = allocator::get_maximum_aligned_size::<VM>(size, align);
    assert!(
        aligned_size <= MAX_BIN_SIZE,
        "FreeListAllocator request (size = {size}, align = {align}, aligned_size = {aligned_size}) exceeds MAX_BIN_SIZE ({MAX_BIN_SIZE})"
    );
    let bin = mi_bin_from_size(aligned_size);
    BinIndex::new(bin)
        .expect("mi_bin_from_size must be <= MAX_BIN for aligned_size <= MAX_BIN_SIZE")
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
    use std::mem::{align_of, offset_of, size_of};

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
            let fin_bin = BinIndex::new(bin).expect("bin must fit in Fin<49>");
            assert_eq!(block_lists[fin_bin].size(), bin_cell_size(fin_bin));
            let (min_size, max_size) = get_bin_size_range(bin, &block_lists).unwrap_or_else(|| {
                panic!("Invalid bin {} for size {}", bin, size);
            });
            assert!(
                (min_size..=max_size).contains(&size),
                "Assigning size={} to bin={} ({:?}) incorrect",
                size,
                bin,
                (min_size, max_size)
            );
        }
    }

    #[test]
    fn test_block_list_repr_c_layout() {
        let word = size_of::<usize>();
        assert_eq!(size_of::<BlockList<SweptBlock>>(), 3 * word);
        assert_eq!(align_of::<BlockList<SweptBlock>>(), word);
        assert_eq!(offset_of!(BlockList<SweptBlock>, first), 0);
        assert_eq!(offset_of!(BlockList<SweptBlock>, last), word);
        assert_eq!(offset_of!(BlockList<SweptBlock>, size), 2 * word);

        assert_eq!(size_of::<BlockList<UnsweptBlock>>(), 3 * word);
        assert_eq!(align_of::<BlockList<UnsweptBlock>>(), word);
        assert_eq!(offset_of!(BlockList<UnsweptBlock>, first), 0);
        assert_eq!(offset_of!(BlockList<UnsweptBlock>, last), word);
        assert_eq!(offset_of!(BlockList<UnsweptBlock>, size), 2 * word);

        assert_eq!(
            size_of::<[BlockList<SweptBlock>; MAX_BIN + 1]>(),
            49 * 3 * word
        );
    }
}
