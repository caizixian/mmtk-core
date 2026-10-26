use super::{Block, BlockState, OwnedBlock};
use crate::util::alloc::allocator;
use crate::util::constants::MIN_OBJECT_SIZE;
use crate::util::linear_scan::Region;
use crate::util::rust_util::fin::Fin;
use crate::util::Address;
use crate::vm::VMBinding;
use std::marker::PhantomData;
use std::ops::Deref;

/// Intrusive singly-linked list of [`OwnedBlock`]s belonging to a single MiMalloc size class.
///
/// # `#[repr(C)]` Memory Layout
/// [`OwnedBlock`] and [`Block`] are `#[repr(transparent)]` over [`std::num::NonZeroUsize`], so
/// `Option<OwnedBlock>` and `Option<Block>` use Rust's guaranteed null-pointer optimization
/// (`0` = `None`, non-zero 64 KiB-aligned address = `Some`) and are pointer-sized:
///
/// | Field | Type | 64-bit Offset | 64-bit Size | 32-bit Offset | 32-bit Size |
/// | :--- | :--- | :---: | :---: | :---: | :---: |
/// | `first` | `Option<OwnedBlock>` | `0` | `8 B` | `0` | `4 B` |
/// | `last` | `Option<Block>` | `8` | `8 B` | `4` | `4 B` |
/// | **Total** | `BlockList` | — | **`16 B` (align `8 B`)** | — | **`8 B` (align `4 B`)** |
///
/// # Representation Invariant
/// 1. Either `self.first.is_none() && self.last.is_none()` (empty list), or
///    `self.first == Some(head) && self.last == Some(tail)` forming a finite, acyclic
///    singly-linked list of distinct [`OwnedBlock`]s via `Block::NEXT_BLOCK_TABLE` (`tail` has
///    `next == None`).
/// 2. Every block in the list upholds [`OwnedBlock`]'s spatial, side-metadata, cell-size, and
///    intrusive free-cell chain invariants, with unique ownership held by `self` (`head` directly
///    in `self.first`, and each successor via the preceding block's `NEXT_BLOCK_TABLE` entry).
#[repr(C)]
pub struct BlockList {
    first: Option<OwnedBlock>,
    last: Option<Block>,
}

static_assertions::assert_eq_size!(BlockList, [usize; 2]);
static_assertions::assert_eq_align!(BlockList, usize);

impl std::fmt::Debug for BlockList {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "BlockList {:?}", self.iter().collect::<Vec<Block>>())
    }
}

impl BlockList {
    pub(crate) const fn new() -> Self {
        Self {
            first: None,
            last: None,
        }
    }

    /// Return `true` if the list contains no blocks.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.first.is_none()
    }

    /// Return the [`Block`] handle of the front block without unlinking it.
    #[inline(always)]
    pub fn first(&self) -> Option<Block> {
        self.first.as_ref().map(OwnedBlock::id)
    }

    /// Pop the front block from the list, returning detached linear ownership.
    pub(crate) fn pop(&mut self) -> Option<OwnedBlock> {
        let head = self.first.take()?;
        let raw_head = head.id();
        if let Some(next) = raw_head.load_next_block() {
            raw_head.store_next_block(None);
            // SAFETY: By `BlockList` invariants (1)-(2), `next` was stored in `raw_head`'s
            // `NEXT_BLOCK_TABLE` link as the uniquely owned successor in `self`'s acyclic chain,
            // and unlinking `raw_head` transfers unique ownership of `next` to `self.first`.
            self.first = Some(unsafe { OwnedBlock::from_block_unchecked(next) });
        } else {
            self.last = None;
        }
        Some(head)
    }

    /// Push a detached `block` to the front of `self` in `O(1)`.
    pub(crate) fn push(&mut self, block: OwnedBlock) {
        let id = block.id();
        if let Some(old_head) = self.first.replace(block) {
            id.store_next_block(Some(old_head.into_raw_for_list()));
        } else {
            id.store_next_block(None);
            self.last = Some(id);
        }
    }

    /// Append a detached `block` to the tail of `self` in `O(1)`.
    fn push_back(&mut self, block: OwnedBlock) {
        let id = block.id();
        id.store_next_block(None);
        if let Some(old_tail) = self.last.replace(id) {
            old_tail.store_next_block(Some(block.into_raw_for_list()));
        } else {
            self.first = Some(block);
        }
    }

    /// Move all blocks of `other` onto the tail of `self` in `O(1)`, leaving `other` empty.
    pub(crate) fn append(&mut self, other: &mut BlockList) {
        if let Some(other_first) = other.first.take() {
            let other_last = other
                .last
                .take()
                .expect("non-empty BlockList must have last");
            if let Some(self_tail) = self.last.replace(other_last) {
                self_tail.store_next_block(Some(other_first.into_raw_for_list()));
            } else {
                self.first = Some(other_first);
            }
        }
    }

    /// Return `true` iff `self` is non-empty and its front block has at least one free cell.
    #[inline]
    pub(crate) fn has_head_free_cells(&self) -> bool {
        self.first.as_ref().is_some_and(OwnedBlock::has_free_cells)
    }

    /// Pop and zero one cell of known `cell_size` from the front block of `self` without unlinking
    /// the block, returning `Address::ZERO` if `self` is empty or its front block has no free cells.
    #[inline(always)]
    pub(super) fn alloc_cell_from_head(&mut self, cell_size: usize) -> Address {
        match self.first.as_mut() {
            Some(head) => head.alloc_cell_known_size(cell_size),
            None => Address::ZERO,
        }
    }

    /// Move all blocks in `self` that currently have free cells to `dest`.
    pub(crate) fn transfer_available_blocks(&mut self, dest: &mut BlockList) {
        let mut kept = BlockList::new();
        while let Some(block) = self.pop() {
            if block.has_free_cells() {
                dest.push(block);
            } else {
                kept.push_back(block);
            }
        }
        *self = kept;
    }

    /// Release all `BlockState::Unmarked` blocks in `self` back to `space`, keeping
    /// `BlockState::Marked` blocks in order.
    pub(crate) fn release_blocks<VM: VMBinding>(&mut self, space: &super::MarkSweepSpace<VM>) {
        let mut kept = BlockList::new();
        while let Some(block) = self.pop() {
            match block.id().get_state() {
                BlockState::Unallocated => unreachable!("Unallocated block in BlockList"),
                BlockState::Unmarked => space.release_block(block),
                BlockState::Marked => kept.push_back(block),
            }
        }
        *self = kept;
    }

    /// Release all `BlockState::Unmarked` blocks in `self` to `space` and move all surviving
    /// `BlockState::Marked` blocks onto `unswept` for lazy sweeping.
    pub(crate) fn release_blocks_and_move_marked_to_unswept<VM: VMBinding>(
        &mut self,
        space: &super::MarkSweepSpace<VM>,
        unswept: &mut BlockList,
    ) {
        while let Some(block) = self.pop() {
            match block.id().get_state() {
                BlockState::Unallocated => unreachable!("Unallocated block in BlockList"),
                BlockState::Unmarked => space.release_block(block),
                BlockState::Marked => unswept.push_back(block),
            }
        }
    }

    /// Return a read-only iterator over the [`Block`] handles in `self`.
    pub fn iter(&self) -> BlockListIterator<'_> {
        BlockListIterator {
            cursor: self.first(),
            _list: PhantomData,
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
        let ret = self.cursor?;
        self.cursor = ret.load_next_block();
        Some(ret)
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

/// Bounded ordinal index `Fin<49>` (`0 <= bin <= MAX_BIN`) into [`BlockLists`].
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

static EMPTY_BLOCK_LISTS: [BlockList; MAX_BIN + 1] = [const { BlockList::new() }; MAX_BIN + 1];

/// Sealed 49-bin array of [`BlockList`]s (`#[repr(transparent)]` over
/// `Option<Box<[BlockList; MAX_BIN + 1]>>`, `1` pointer word inline, `784 B` on heap on 64-bit
/// when allocated, `0 B` on heap for unconfigured [`crate::util::alloc::FreeListAllocator`] slots).
#[repr(transparent)]
#[derive(Debug)]
pub struct BlockLists(Option<Box<[BlockList; MAX_BIN + 1]>>);

static_assertions::assert_eq_size!(BlockLists, usize);
static_assertions::assert_eq_align!(BlockLists, usize);

impl Deref for BlockLists {
    type Target = [BlockList; MAX_BIN + 1];

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        match &self.0 {
            Some(bins) => bins,
            None => &EMPTY_BLOCK_LISTS,
        }
    }
}

impl BlockLists {
    /// Create an unallocated empty `BlockLists` handle (`None`, `0` heap allocation) for
    /// unconfigured allocator slots.
    #[inline(always)]
    pub(crate) const fn uninit() -> Self {
        Self(None)
    }

    /// Allocate a fresh boxed array of 49 empty [`BlockList`]s.
    pub(crate) fn new() -> Self {
        Self(Some(Box::new(std::array::from_fn(|_| BlockList::new()))))
    }

    #[inline(always)]
    fn bins_mut(&mut self) -> &mut [BlockList; MAX_BIN + 1] {
        self.0
            .as_deref_mut()
            .expect("BlockLists is uninitialized (unconfigured FreeListAllocator slot)")
    }

    /// Push `block` onto the front of `bin`.
    #[inline]
    pub(crate) fn push(&mut self, bin: BinIndex, block: OwnedBlock) {
        let expected = bin_cell_size(bin);
        let actual = block.cell_size();
        assert_eq!(
            actual, expected,
            "BlockLists::push cell size mismatch: block has {actual} B cells, bin {bin:?} expects {expected} B"
        );
        self.bins_mut()[bin].push(block);
    }

    /// Pop the front [`OwnedBlock`] from `bin`.
    #[inline]
    pub(crate) fn pop(&mut self, bin: BinIndex) -> Option<OwnedBlock> {
        self.bins_mut()[bin].pop()
    }

    /// Pop and zero one cell from the front block of `bin` without unlinking the block.
    #[inline(always)]
    pub(crate) fn alloc_cell_from_head(&mut self, bin: BinIndex) -> Address {
        self.bins_mut()[bin].alloc_cell_from_head(bin_cell_size(bin))
    }

    /// Move all blocks from `other[bin]` onto the tail of `self[bin]`.
    #[inline]
    pub(crate) fn append_bin_from(&mut self, bin: BinIndex, other: &mut Self) {
        self.bins_mut()[bin].append(&mut other.bins_mut()[bin]);
    }

    /// Move all blocks in `self[bin]` that currently have free cells to `dest[bin]`.
    #[inline]
    pub(crate) fn transfer_available_bin_to(&mut self, bin: BinIndex, dest: &mut Self) {
        self.bins_mut()[bin].transfer_available_blocks(&mut dest.bins_mut()[bin]);
    }

    /// Release all `BlockState::Unmarked` blocks in `self[bin]` back to `space`.
    #[inline]
    pub(crate) fn release_blocks<VM: VMBinding>(
        &mut self,
        bin: BinIndex,
        space: &super::MarkSweepSpace<VM>,
    ) {
        self.bins_mut()[bin].release_blocks(space);
    }

    /// Release all `BlockState::Unmarked` blocks in `self[bin]` to `space` and move surviving
    /// `BlockState::Marked` blocks onto `unswept[bin]` for lazy sweeping.
    #[inline]
    pub(crate) fn release_blocks_and_move_marked_to_unswept<VM: VMBinding>(
        &mut self,
        bin: BinIndex,
        space: &super::MarkSweepSpace<VM>,
        unswept: &mut Self,
    ) {
        self.bins_mut()[bin]
            .release_blocks_and_move_marked_to_unswept(space, &mut unswept.bins_mut()[bin]);
    }
}

/// Create an empty set of [`BlockList`]s of different size classes (bins).
pub(crate) fn new_empty_block_lists() -> BlockLists {
    BlockLists::new()
}

/// Align a byte size to a size in machine words
/// i.e. byte size == `wsize*sizeof(void*)`
/// adapted from _mi_wsize_from_size in mimalloc
#[inline(always)]
fn mi_wsize_from_size(size: usize) -> usize {
    size.div_ceil(MI_INTPTR_SIZE)
}

/// Compute the bounded [`BinIndex`] (`Fin<49>`, `1 <= bin <= MAX_BIN`) for `(size, align)`,
/// panicking if the maximum aligned allocation size exceeds [`MAX_BIN_SIZE`].
#[inline(always)]
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

#[inline(always)]
fn mi_bin_from_size(size: usize) -> usize {
    // adapted from _mi_bin in mimalloc
    let mut wsize: usize = mi_wsize_from_size(size);
    debug_assert!(wsize <= MI_LARGE_OBJ_WSIZE_MAX);
    if wsize <= 1 {
        1
    } else if wsize <= 8 {
        wsize
    } else {
        wsize -= 1;
        let b = (MI_INTPTR_BITS - 1 - usize::leading_zeros(wsize) as usize) as u8;
        (((b << 2) + ((wsize >> (b - 2)) & 0x03) as u8) - 3) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, offset_of, size_of};

    fn get_bin_size_range(bin: usize) -> Option<(usize, usize)> {
        if bin == 0 || bin > MAX_BIN {
            None
        } else if bin == 1 {
            Some((0, BIN_CELL_SIZES[1]))
        } else {
            Some((BIN_CELL_SIZES[bin - 1], BIN_CELL_SIZES[bin]))
        }
    }

    #[test]
    fn test_mi_bin() {
        for size in 0..=MAX_BIN_SIZE {
            let bin = mi_bin_from_size(size);
            let fin_bin = BinIndex::new(bin).expect("bin must fit in Fin<49>");
            assert_eq!(BIN_CELL_SIZES[bin], bin_cell_size(fin_bin));
            let (min_size, max_size) = get_bin_size_range(bin).unwrap_or_else(|| {
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
        assert_eq!(size_of::<BlockList>(), 2 * word);
        assert_eq!(align_of::<BlockList>(), word);
        assert_eq!(offset_of!(BlockList, first), 0);
        assert_eq!(offset_of!(BlockList, last), word);
        assert_eq!(size_of::<[BlockList; MAX_BIN + 1]>(), 49 * 2 * word);
        assert_eq!(size_of::<BlockLists>(), word);
    }
}
