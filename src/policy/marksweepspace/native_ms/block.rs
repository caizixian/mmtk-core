// adapted from Immix
use std::num::NonZeroUsize;
use std::sync::atomic::Ordering;

use crate::util::constants::{BYTES_IN_ADDRESS, LOG_BYTES_IN_PAGE, MIN_OBJECT_SIZE};
use crate::util::heap::chunk_map::Chunk;
use crate::util::linear_scan::{Region, UnstraddlableRegion};
use crate::util::metadata::side_metadata::SideMetadataSpec;
use crate::util::object_enum::BlockMayHaveObjects;
use crate::util::{Address, ObjectReference};
use crate::vm::{ObjectModel, VMBinding};

use super::block_list::{bin_cell_size, BinIndex};

/// A 64 KiB region for MiMalloc.
///
/// This is also known as a MiMalloc page. We call it `Block` to avoid confusion with OS 4 KiB pages.
/// Backed by [`NonZeroUsize`] (`#[repr(transparent)]`), guaranteeing:
/// 1. Non-zero 64 KiB alignment (`size_of::<Option<Block>>() == size_of::<usize>()` via NPO), and
/// 2. Provenance-preserving `Address` round-trip (`Address` wraps exposed-provenance `usize`).
#[derive(Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Block(NonZeroUsize);

static_assertions::assert_eq_size!(Block, usize);
static_assertions::assert_eq_size!(Option<Block>, usize);
static_assertions::assert_eq_align!(Option<Block>, usize);

impl std::fmt::Debug for Block {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Block(0x{:x})", self.0.get())
    }
}

impl Region for Block {
    const LOG_BYTES: usize = 16;

    fn from_aligned_address(address: Address) -> Self {
        assert!(
            address.is_aligned_to(Self::BYTES),
            "native_ms::Block address {address} is not aligned to {:#x}",
            Self::BYTES
        );
        let nz = NonZeroUsize::new(address.as_usize())
            .expect("native_ms::Block address must be non-zero");
        Self(nz)
    }

    #[inline(always)]
    fn start(&self) -> Address {
        // SAFETY: `self.0` was validated in `Block::from_aligned_address` from a non-zero,
        // 64 KiB-aligned `Address` (preserving its exposed-provenance integer representation).
        unsafe { Address::from_usize(self.0.get()) }
    }
}

/// Objects cannot straddle multiple native MarkSweep blocks.
impl UnstraddlableRegion for Block {}

impl BlockMayHaveObjects for Block {
    fn may_have_objects(&self) -> bool {
        self.get_state() != BlockState::Unallocated
    }
}

impl Block {
    /// Log pages in block
    pub const LOG_PAGES: usize = Self::LOG_BYTES - LOG_BYTES_IN_PAGE as usize;

    pub const METADATA_SPECS: [SideMetadataSpec; 4] = [
        Self::MARK_TABLE,
        Self::NEXT_BLOCK_TABLE,
        Self::FREE_LIST_TABLE,
        Self::SIZE_TABLE,
    ];

    /// Block mark table (side)
    pub const MARK_TABLE: SideMetadataSpec =
        crate::util::metadata::side_metadata::spec_defs::MS_BLOCK_MARK;

    /// Singly-linked next-block pointer in [`super::BlockList`] (side)
    pub const NEXT_BLOCK_TABLE: SideMetadataSpec =
        crate::util::metadata::side_metadata::spec_defs::MS_BLOCK_NEXT;

    /// Head cell address of this block's intrusive free-cell chain (side)
    pub const FREE_LIST_TABLE: SideMetadataSpec =
        crate::util::metadata::side_metadata::spec_defs::MS_FREE;

    #[cfg(feature = "malloc_native_mimalloc")]
    pub const LOCAL_FREE_LIST_TABLE: SideMetadataSpec =
        crate::util::metadata::side_metadata::spec_defs::MS_LOCAL_FREE;

    #[cfg(feature = "malloc_native_mimalloc")]
    pub const THREAD_FREE_LIST_TABLE: SideMetadataSpec =
        crate::util::metadata::side_metadata::spec_defs::MS_THREAD_FREE;

    /// Cell size in bytes for this block (side)
    pub const SIZE_TABLE: SideMetadataSpec =
        crate::util::metadata::side_metadata::spec_defs::MS_BLOCK_SIZE;

    #[inline(always)]
    fn load_address_meta(&self, spec: SideMetadataSpec) -> Address {
        let raw = spec.load_atomic::<usize>(self.start(), Ordering::Relaxed);
        // SAFETY: `Address(usize)` wraps the raw address integer stored in `spec` (`0` or a valid
        // block/cell address); callers validate alignment and bounds before dereferencing.
        unsafe { Address::from_usize(raw) }
    }

    #[inline(always)]
    pub(super) fn load_next_block(&self) -> Option<Block> {
        let addr = self.load_address_meta(Self::NEXT_BLOCK_TABLE);
        if addr.is_zero() {
            None
        } else {
            Some(Self::from_aligned_address(addr))
        }
    }

    #[inline(always)]
    pub(super) fn store_next_block(&self, next: Option<Block>) {
        let raw = next.map_or(0, |b| b.start().as_usize());
        Self::NEXT_BLOCK_TABLE.store_atomic::<usize>(self.start(), raw, Ordering::Relaxed);
    }

    #[inline(always)]
    fn load_free_list(&self) -> Address {
        self.load_address_meta(Self::FREE_LIST_TABLE)
    }

    #[inline(always)]
    fn store_free_list(&self, free_list: Address) {
        Self::FREE_LIST_TABLE.store_atomic::<usize>(
            self.start(),
            free_list.as_usize(),
            Ordering::Relaxed,
        );
    }

    #[inline(always)]
    pub fn load_block_cell_size(&self) -> usize {
        Self::SIZE_TABLE.load_atomic::<usize>(self.start(), Ordering::Relaxed)
    }

    #[inline(always)]
    fn store_block_cell_size(&self, size: usize) {
        debug_assert_ne!(size, 0);
        Self::SIZE_TABLE.store_atomic::<usize>(self.start(), size, Ordering::Relaxed);
    }

    /// Get block mark state.
    #[inline(always)]
    pub fn get_state(&self) -> BlockState {
        BlockState::from(Self::MARK_TABLE.load_atomic::<u8>(self.start(), Ordering::SeqCst))
    }

    /// Set block mark state.
    #[inline(always)]
    pub(super) fn set_state(&self, state: BlockState) {
        Self::MARK_TABLE.store_atomic::<u8>(self.start(), u8::from(state), Ordering::SeqCst);
    }

    /// Sweep a `BlockState::Marked` block, rebuilding its intrusive free-cell chain from side mark
    /// bits and transitioning its state to `BlockState::Unmarked`.
    ///
    /// # Safety
    /// - `self` must belong to an allocated `MarkSweepSpace` chunk with mapped heap pages
    ///   `[self.start(), self.end())`, mapped local metadata (`SIZE_TABLE`, `FREE_LIST_TABLE`,
    ///   `LOCAL_MARK_BIT_SPEC`, and `VO_BIT` when enabled), and a valid MiMalloc bin cell size in
    ///   `SIZE_TABLE` (`MIN_OBJECT_SIZE <= cs <= Block::BYTES` and `cs % BYTES_IN_ADDRESS == 0`).
    /// - The caller must have exclusive access to `self`'s free-list metadata and unmarked cells
    ///   after STW trace completion (so every live object in `self` has its side mark bit set).
    pub(super) unsafe fn sweep_cells<VM: VMBinding>(&self) {
        if cfg!(feature = "malloc_native_mimalloc") {
            unimplemented!()
        }

        assert_eq!(
            self.get_state(),
            BlockState::Marked,
            "native_ms::Block::sweep_cells called on non-Marked block {:?}",
            self
        );
        assert!(
            VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.is_on_side(),
            "MarkSweepSpace requires side mark bits"
        );
        const _: () = assert!(MIN_OBJECT_SIZE == ObjectReference::ALIGNMENT);

        let mark_side = VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.extract_side_spec();
        let cell_size = self.load_block_cell_size();
        assert!(
            (MIN_OBJECT_SIZE..=Block::BYTES).contains(&cell_size)
                && cell_size % BYTES_IN_ADDRESS == 0,
            "Invalid cell size {cell_size} in {:?}",
            self
        );

        // When cell start addresses equal object reference addresses for `VM`, a single mark-bit
        // load at `cell` suffices; otherwise scan candidate object-reference alignments in `cell`.
        let simple_cell_mark = !VM::USE_ALLOCATION_OFFSET
            && VM::MAX_ALIGNMENT == VM::MIN_ALIGNMENT
            && crate::util::conversions::raw_is_aligned(cell_size, VM::MAX_ALIGNMENT)
            && VM::VMObjectModel::UNIFIED_OBJECT_REFERENCE_ADDRESS;
        let lower_offset =
            (VM::VMObjectModel::OBJECT_REF_OFFSET_LOWER_BOUND.max(0) as usize).min(cell_size);

        let mut cell = self.start();
        let block_end = self.end();
        let mut last = Address::ZERO;

        while cell + cell_size <= block_end {
            let cell_end = cell + cell_size;
            let is_live = if simple_cell_mark {
                mark_side.load_atomic::<u8>(cell, Ordering::SeqCst) != 0
            } else {
                let mut candidate = (cell + lower_offset).align_up(ObjectReference::ALIGNMENT);
                let mut found = false;
                while candidate < cell_end {
                    if mark_side.load_atomic::<u8>(candidate, Ordering::SeqCst) != 0 {
                        found = true;
                        break;
                    }
                    candidate += MIN_OBJECT_SIZE;
                }
                found
            };

            if !is_live {
                #[cfg(feature = "vo_bit")]
                if simple_cell_mark {
                    crate::util::metadata::vo_bit::VO_BIT_SIDE_METADATA_SPEC.store_atomic::<u8>(
                        cell,
                        0,
                        Ordering::SeqCst,
                    );
                } else {
                    crate::util::metadata::vo_bit::bzero_vo_bit(cell, cell_size);
                }

                // SAFETY: `cell` is word-aligned (`self.start()` is 64 KiB-aligned and `cell_size`
                // is a non-zero multiple of `BYTES_IN_ADDRESS`), `[cell, cell_end)` lies within
                // mapped block memory `[self.start(), block_end)`, and no mark bit was set in this
                // cell so it is dead and exclusively owned by the sweeping thread.
                unsafe {
                    cell.store::<Address>(last);
                }
                last = cell;
            }

            cell = cell_end;
        }

        self.store_free_list(last);
        self.set_state(BlockState::Unmarked);
    }

    /// Get the chunk containing the block.
    pub fn chunk(&self) -> Chunk {
        Chunk::from_unaligned_address(self.start())
    }

    /// Initialize a clean block after acquired from page-resource.
    pub(super) fn init(&self) {
        self.set_state(BlockState::Unmarked);
    }

    /// Deinitialize a block before releasing.
    pub(super) fn deinit(&self) {
        self.set_state(BlockState::Unallocated);
    }
}

/// Linear (`!Copy, !Clone`) ownership of a mapped 64 KiB `MarkSweepSpace` block.
///
/// # Representation Invariant
/// 1. `self.0` belongs to an allocated `MarkSweepSpace` chunk with mapped heap pages
///    `[self.start(), self.end())` and mapped side metadata (`Block::METADATA_SPECS`,
///    `LOCAL_MARK_BIT_SPEC`, and `VO_BIT` when enabled).
/// 2. Unique ownership: each allocated block is owned by at most one detached `OwnedBlock` or one
///    [`super::BlockList`] chain at a time (and never mutated outside `SweepChunk` while STW eager
///    sweeping is active).
/// 3. `cs = SIZE_TABLE(self.0)` is a valid MiMalloc bin cell size (`MIN_OBJECT_SIZE <= cs <= Block::BYTES`
///    and `cs % BYTES_IN_ADDRESS == 0`), and `FREE_LIST_TABLE(self.0)` heads a finite, acyclic
///    `0`-terminated chain of disjoint, word-aligned unallocated/dead cells
///    `[c_i, c_i + cs) \subseteq [self.start(), self.end())`.
#[repr(transparent)]
pub struct OwnedBlock(Block);

static_assertions::assert_eq_size!(OwnedBlock, usize);
static_assertions::assert_eq_size!(Option<OwnedBlock>, usize);
static_assertions::assert_eq_align!(Option<OwnedBlock>, usize);

impl std::fmt::Debug for OwnedBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("OwnedBlock").field(&self.0).finish()
    }
}

impl OwnedBlock {
    /// Reconstruct an `OwnedBlock` from its underlying `Block` handle.
    ///
    /// # Safety
    /// Caller must uphold `OwnedBlock` invariants (1)-(3) (or immediately call
    /// [`Self::init_free_cells`] on a freshly acquired unaliased block before exposing `self`).
    #[inline(always)]
    pub(super) unsafe fn from_block_unchecked(block: Block) -> Self {
        Self(block)
    }

    /// Consume `self` into its raw `Block` handle when linking `self` inside a [`super::BlockList`].
    #[inline(always)]
    pub(super) fn into_raw_for_list(self) -> Block {
        self.0
    }

    /// Return the underlying [`Block`] ID (`Copy`) for read-only metadata queries.
    #[inline(always)]
    pub(crate) fn id(&self) -> Block {
        self.0
    }

    /// Return the block start address.
    #[inline(always)]
    pub(crate) fn start(&self) -> Address {
        self.0.start()
    }

    /// Return the cell size (in bytes) recorded for this block.
    #[inline(always)]
    pub(crate) fn cell_size(&self) -> usize {
        self.0.load_block_cell_size()
    }

    /// Initialize the intrusive free-cell chain of a freshly acquired empty block for `bin`.
    pub(super) fn init_free_cells(&mut self, bin: BinIndex) {
        let cell_size = bin_cell_size(bin);
        debug_assert!((MIN_OBJECT_SIZE..=Block::BYTES).contains(&cell_size));
        debug_assert_eq!(cell_size % BYTES_IN_ADDRESS, 0);

        let block = self.0;
        let block_end = block.end();
        let mut prev_cell = Address::ZERO;
        let mut cell = block.start();

        let head_cell = loop {
            // SAFETY: `self` uniquely owns this freshly acquired, mapped 64 KiB block with no live
            // objects, and `bin_cell_size(bin)` is statically verified to be a non-zero multiple of
            // `BYTES_IN_ADDRESS` (`<= Block::BYTES`), so `cell` is word-aligned and in bounds.
            unsafe {
                cell.store::<Address>(prev_cell);
            }
            prev_cell = cell;
            cell += cell_size;
            if cell + cell_size > block_end {
                break prev_cell;
            }
        };

        block.store_free_list(head_cell);
        block.store_block_cell_size(cell_size);
        block.store_next_block(None);
        #[cfg(feature = "malloc_native_mimalloc")]
        {
            Block::LOCAL_FREE_LIST_TABLE.set_zero_atomic(block.start(), Ordering::Relaxed);
            Block::THREAD_FREE_LIST_TABLE.set_zero_atomic(block.start(), Ordering::Relaxed);
        }
    }

    /// Return `true` iff this block's intrusive free list has at least one free cell.
    #[inline(always)]
    pub(crate) fn has_free_cells(&self) -> bool {
        !self.0.load_free_list().is_zero()
    }

    /// Pop and zero one cell of known `cell_size` (`SIZE_TABLE(self.0) == cell_size`, capsule-
    /// enforced by `BlockLists::push` on an `Unmarked` available block), returning `Address::ZERO`
    /// if empty.
    #[inline(always)]
    pub(super) fn alloc_cell_known_size(&mut self, cell_size: usize) -> Address {
        debug_assert_eq!(
            self.0.get_state(),
            BlockState::Unmarked,
            "Cannot allocate from unswept or unallocated block {:?}",
            self.0
        );
        debug_assert_eq!(self.0.load_block_cell_size(), cell_size);
        let cell = self.0.load_free_list();
        if cell.is_zero() {
            return Address::ZERO;
        }
        // SAFETY: `OwnedBlock` invariant (3) guarantees non-zero `cell` is a uniquely owned,
        // word-aligned dead cell `[cell, cell + cs)` inside mapped `[self.start(), self.end())`
        // whose first word holds the next intrusive free-list link.
        let next_cell = unsafe { cell.load::<Address>() };
        debug_assert!(
            next_cell.is_zero() || self.0.includes_address(next_cell),
            "next_cell {} is not in {:?}",
            next_cell,
            self.0
        );
        self.0.store_free_list(next_cell);
        crate::util::memory::zero(cell, cell_size);
        cell
    }

    /// Rebuild this `BlockState::Marked` block's free-cell chain after STW tracing (`Marked -> Unmarked`).
    #[inline]
    pub(crate) fn sweep<VM: VMBinding>(&mut self) {
        // SAFETY: `OwnedBlock` invariants (1)-(3) guarantee mapped block/metadata pages, valid
        // `SIZE_TABLE`, and unique ownership of `self.0` outside STW tracing/eager chunk sweeping;
        // `sweep_cells` asserts `self.0.get_state() == BlockState::Marked` (established only by STW
        // tracing and cleared to `Unmarked` on return, making double-sweep impossible).
        unsafe {
            self.0.sweep_cells::<VM>();
        }
    }
}

/// The block allocation state.
#[repr(u8)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum BlockState {
    /// the block is not allocated.
    Unallocated = 0,
    /// the block is allocated and swept / unmarked (`u8::MAX`).
    Unmarked = 0xFF,
    /// the block is allocated and marked during GC, awaiting sweep (`u8::MAX - 1`).
    Marked = 0xFE,
}

const _: () = {
    assert!(BlockState::Unmarked as u8 == u8::MAX);
    assert!(BlockState::Marked as u8 == u8::MAX - 1);
};

impl From<u8> for BlockState {
    #[inline(always)]
    fn from(raw: u8) -> Self {
        match raw {
            0 => BlockState::Unallocated,
            0xFF => BlockState::Unmarked,
            0xFE => BlockState::Marked,
            _ => panic!("Invalid native_ms::BlockState discriminant: {raw:#x}"),
        }
    }
}

impl From<BlockState> for u8 {
    #[inline(always)]
    fn from(state: BlockState) -> Self {
        state as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_round_trip_and_npo() {
        #[repr(align(65536))]
        struct MockBlockBuf([u64; Block::BYTES / 8]);

        let mut buf = Box::new(MockBlockBuf([0u64; Block::BYTES / 8]));
        let base = Address::from_mut_ptr(buf.0.as_mut_ptr());
        let block = Block::from_aligned_address(base);
        assert_eq!(block.start(), base);
        assert_eq!(block.end(), base + Block::BYTES);
        assert_eq!(Some(block).map(|b| b.start()), Some(base));
        assert_eq!(block.start().to_mut_ptr::<u64>(), buf.0.as_mut_ptr());
    }

    #[test]
    #[should_panic(expected = "native_ms::Block address must be non-zero")]
    fn test_block_zero_address_panics() {
        let _ = Block::from_aligned_address(Address::ZERO);
    }

    #[test]
    fn test_block_state_round_trip() {
        for state in [
            BlockState::Unallocated,
            BlockState::Unmarked,
            BlockState::Marked,
        ] {
            let raw = u8::from(state);
            assert_eq!(BlockState::from(raw), state);
        }
        assert_eq!(u8::from(BlockState::Unallocated), 0);
    }

    #[test]
    #[should_panic(expected = "Invalid native_ms::BlockState discriminant")]
    fn test_block_state_invalid_discriminant_panics() {
        let _ = BlockState::from(1u8);
    }
}
