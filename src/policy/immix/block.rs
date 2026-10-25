#![deny(unsafe_op_in_unsafe_fn)]

use super::defrag::Histogram;
use super::line::{iter_line_indices, Line, RCArray, BYTES_PER_LINE, LINES_IN_BLOCK};
use super::ImmixSpace;
use crate::util::constants::*;
use crate::util::heap::blockpageresource::BlockPool;
use crate::util::heap::chunk_map::Chunk;
use crate::util::linear_scan::{Region, RegionIterator, UnstraddlableRegion};
use crate::util::metadata::side_metadata::*;
#[cfg(feature = "vo_bit")]
use crate::util::metadata::vo_bit;
#[cfg(feature = "object_pinning")]
use crate::util::metadata::MetadataSpec;
use crate::util::object_enum::BlockMayHaveObjects;
use crate::util::rust_util::fin::Fin;
use crate::util::{Address, ObjectReference};
use crate::vm::*;
use bytemuck::NoUninit;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU8, Ordering};

/// The block allocation state.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum BlockState {
    /// the block is not allocated.
    Unallocated,
    /// the block is a young block.
    Nursery,
    /// the block is allocated but not marked.
    Unmarked,
    /// the block is allocated and marked.
    Marked,
    /// RC mutator recycled blocks.
    Reusing,
    /// the block is marked as reusable.
    Reusable { unavailable_lines: u8 },
}

impl BlockState {
    /// Private constant
    const MARK_UNALLOCATED: u8 = 0;
    /// Private constant
    const MARK_UNMARKED: u8 = u8::MAX;
    /// Private constant
    const MARK_MARKED: u8 = u8::MAX - 1;
    const MARK_NURSERY: u8 = u8::MAX - 2;
    const MARK_REUSING: u8 = u8::MAX - 3;
}

impl From<u8> for BlockState {
    fn from(state: u8) -> Self {
        match state {
            Self::MARK_UNALLOCATED => BlockState::Unallocated,
            Self::MARK_UNMARKED => BlockState::Unmarked,
            Self::MARK_MARKED => BlockState::Marked,
            Self::MARK_NURSERY => BlockState::Nursery,
            Self::MARK_REUSING => BlockState::Reusing,
            unavailable_lines => BlockState::Reusable { unavailable_lines },
        }
    }
}

impl From<BlockState> for u8 {
    fn from(state: BlockState) -> Self {
        match state {
            BlockState::Unallocated => BlockState::MARK_UNALLOCATED,
            BlockState::Unmarked => BlockState::MARK_UNMARKED,
            BlockState::Marked => BlockState::MARK_MARKED,
            BlockState::Nursery => BlockState::MARK_NURSERY,
            BlockState::Reusing => BlockState::MARK_REUSING,
            BlockState::Reusable { unavailable_lines } => {
                assert_ne!(unavailable_lines, 0);
                u8::min(unavailable_lines, u8::MAX - 4)
            }
        }
    }
}

impl MetadataCodec<u8> for BlockState {
    #[inline(always)]
    fn encode(self) -> u8 {
        u8::from(self)
    }

    #[inline(always)]
    fn decode(raw: u8) -> Self {
        Self::from(raw)
    }
}

impl BlockState {
    /// Test if the block is reuasable.
    pub const fn is_reusable(&self) -> bool {
        matches!(self, BlockState::Reusable { .. })
    }
}

/// Data structure to reference an immix block.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialOrd, PartialEq, Eq, NoUninit)]
pub struct Block(Address);

impl Region for Block {
    #[cfg(not(feature = "immix_smaller_block"))]
    const LOG_BYTES: usize = 15;
    // 8K, or one page if the page is larger. A block cannot be smaller than a page.
    #[cfg(feature = "immix_smaller_block")]
    const LOG_BYTES: usize = if 13 > LOG_BYTES_IN_PAGE as usize {
        13
    } else {
        LOG_BYTES_IN_PAGE as usize
    };

    fn from_aligned_address(address: Address) -> Self {
        assert!(address.is_aligned_to(Self::BYTES));
        Self(address)
    }

    fn start(&self) -> Address {
        self.0
    }
}

/// An objects cannot straddle multiple Immix blocks.
impl UnstraddlableRegion for Block {}

impl BlockMayHaveObjects for Block {
    fn may_have_objects(&self) -> bool {
        self.get_state() != BlockState::Unallocated
    }
}

impl Block {
    /// Log pages in block
    pub const LOG_PAGES: usize = Self::LOG_BYTES - LOG_BYTES_IN_PAGE as usize;
    /// Pages in block
    pub const PAGES: usize = 1 << Self::LOG_PAGES;
    /// Log lines in block
    pub const LOG_LINES: usize = Self::LOG_BYTES - Line::LOG_BYTES;
    /// Lines in block
    pub const LINES: usize = 1 << Self::LOG_LINES;

    /// Block defrag state table (side)
    pub const DEFRAG_STATE_TABLE: SideMetadataSpec =
        crate::util::metadata::side_metadata::spec_defs::IX_BLOCK_DEFRAG;

    /// Block mark table (side)
    pub const MARK_TABLE: SideMetadataSpec =
        crate::util::metadata::side_metadata::spec_defs::IX_BLOCK_MARK;
    pub const LOG_TABLE: SideMetadataSpec =
        crate::util::metadata::side_metadata::spec_defs::IX_BLOCK_LOG;
    pub const NURSERY_PROMOTION_STATE_TABLE: SideMetadataSpec =
        crate::util::metadata::side_metadata::spec_defs::NURSERY_PROMOTION_STATE;

    const TYPED_DEFRAG_STATE_TABLE: TypedSideSpec<Block, U8Bits, u8> =
        TypedSideSpec::new(Self::DEFRAG_STATE_TABLE);
    const TYPED_MARK_TABLE: TypedSideSpec<Block, U8Bits, BlockState> =
        TypedSideSpec::new(Self::MARK_TABLE);
    const TYPED_LOG_TABLE: TypedSideSpec<Block, U1Bit, bool> = TypedSideSpec::new(Self::LOG_TABLE);
    const TYPED_NURSERY_PROMOTION_TABLE: TypedSideSpec<Block, U8Bits, bool> =
        TypedSideSpec::new(Self::NURSERY_PROMOTION_STATE_TABLE);

    /// Checked projection of `RC_TABLE` for plan-level callers (`calc_dead_lines`, `clear_rc_table`)
    /// that do not pass `&ImmixSpace<VM>`.
    fn rc_array_checked(&self) -> RCArray<'_> {
        assert!(
            !self.is_zero()
                && self.start().is_aligned_to(Self::BYTES)
                && self.start().as_usize() <= usize::MAX - Self::BYTES
                && crate::util::rc::RC_TABLE.is_mapped(self.start())
                && crate::util::rc::RC_TABLE.is_mapped(self.end() - 1usize),
            "Block {:?} is not a non-zero aligned Immix block with mapped RC_TABLE",
            self
        );
        // SAFETY:
        // 1. The assertion above unconditionally verifies `!self.is_zero()`, `self.start()` is
        //    aligned to `Self::BYTES` (`<= 32 KiB << BYTES_IN_CHUNK`) without overflow, and global
        //    contiguous `RC_TABLE` is mapped across `[self.start(), self.end())` (within a single
        //    `MMAPPER` page, which is zero-initialized and never unmapped for the process lifetime).
        // 2. Every safe `RC_TABLE` operation in the crate (`RefCountHelper`, `SideMetadataSpec`
        //    atomic ops, `RCArray`) accesses `RC_TABLE` exclusively via `AtomicU8`; the only
        //    non-atomic `RC_TABLE` accessors (`set_relaxed`, `set_line_relaxed`,
        //    `object_or_line_is_dead`, `rc_table_range`) are `unsafe fn`s whose own `# Safety`
        //    contracts require their callers to guarantee exclusivity.
        let table = unsafe {
            crate::util::rc::RC_TABLE
                .mapped_atomic_byte_chunks_unchecked::<LINES_IN_BLOCK, BYTES_PER_LINE>(self.start())
        };
        RCArray::from_table(*self, table)
    }

    pub fn calc_dead_lines(&self) -> usize {
        let rc_array = self.rc_array_checked();
        iter_line_indices()
            .filter(|&idx| rc_array.is_dead(idx))
            .count()
    }

    pub const ZERO: Self = Self(Address::ZERO);

    #[allow(unused)]
    pub fn is_zero(&self) -> bool {
        self.0.is_zero()
    }

    /// Get the chunk containing the block.
    pub fn chunk(&self) -> Chunk {
        Chunk::from_unaligned_address(self.0)
    }

    /// Get block mark state.
    pub fn get_state(&self) -> BlockState {
        Self::TYPED_MARK_TABLE.load_atomic(*self, Ordering::SeqCst)
    }

    /// Set block mark state.
    pub fn set_state(&self, state: BlockState) {
        Self::TYPED_MARK_TABLE.store_atomic(*self, state, Ordering::SeqCst);
    }

    /// Set block mark state.
    pub fn fetch_update_state(
        &self,
        mut f: impl FnMut(BlockState) -> Option<BlockState>,
    ) -> Result<BlockState, BlockState> {
        Self::MARK_TABLE
            .fetch_update_atomic::<u8, _>(self.start(), Ordering::SeqCst, Ordering::SeqCst, |s| {
                f(s.into()).map(u8::from)
            })
            .map(BlockState::from)
            .map_err(BlockState::from)
    }

    pub fn attempt_dealloc(&self, ignore_reusing_blocks: bool) -> bool {
        self.fetch_update_state(|s| {
            if (ignore_reusing_blocks && s == BlockState::Reusing) || s == BlockState::Unallocated {
                None
            } else {
                Some(BlockState::Unallocated)
            }
        })
        .is_ok()
    }

    // Defrag byte

    const DEFRAG_SOURCE_STATE: u8 = u8::MAX;

    /// Test if the block is marked for defragmentation.
    pub fn is_defrag_source(&self) -> bool {
        let byte = Self::TYPED_DEFRAG_STATE_TABLE.load_atomic(*self, Ordering::SeqCst);
        // The byte should be 0 (not defrag source) or 255 (defrag source) if this is a major defrag GC, as we set the values in PrepareBlockState.
        // But it could be any value in a nursery GC.
        byte == Self::DEFRAG_SOURCE_STATE
    }

    pub fn in_defrag_block(o: ObjectReference) -> bool {
        Block::containing(o).is_defrag_source()
    }

    pub fn address_in_defrag_block(a: Address) -> bool {
        Block::from_unaligned_address(a).is_defrag_source()
    }

    /// Mark the block for defragmentation.
    pub fn set_as_defrag_source(&self, defrag: bool) {
        let byte = if defrag { Self::DEFRAG_SOURCE_STATE } else { 0 };
        Self::TYPED_DEFRAG_STATE_TABLE.store_atomic(*self, byte, Ordering::SeqCst);
    }

    /// Record the number of holes in the block.
    pub fn set_holes(&self, holes: usize) {
        Self::TYPED_DEFRAG_STATE_TABLE.store_atomic(*self, holes as u8, Ordering::SeqCst);
    }

    /// Get the number of holes.
    pub fn get_holes(&self) -> usize {
        let byte = Self::TYPED_DEFRAG_STATE_TABLE.load_atomic(*self, Ordering::SeqCst);
        debug_assert_ne!(byte, Self::DEFRAG_SOURCE_STATE);
        byte as usize
    }

    /// Initialize a clean block after acquired from page-resource.
    pub fn init<VM: VMBinding>(&self, copy: bool, reuse: bool, space: &ImmixSpace<VM>) {
        if space.rc_enabled {
            if !reuse {
                debug_assert_eq!(self.get_state(), BlockState::Unallocated);
            }
            self.clear_in_place_promoted();
            if !copy && reuse {
                self.set_state(BlockState::Reusing);
                debug_assert!(!self.is_defrag_source());
            } else if copy {
                if reuse {
                    debug_assert!(!self.is_defrag_source());
                }
                self.set_state(BlockState::Unmarked);
                self.set_as_defrag_source(false);
            } else {
                self.set_state(BlockState::Nursery);
                self.set_as_defrag_source(false);
            }
        } else {
            self.set_state(if copy {
                BlockState::Marked
            } else {
                BlockState::Unmarked
            });
            if !reuse {
                Self::TYPED_DEFRAG_STATE_TABLE.store_atomic(*self, 0, Ordering::SeqCst);
            }
        }
    }

    /// Deinitalize a block before releasing.
    pub fn deinit<VM: VMBinding>(&self, space: &ImmixSpace<VM>) {
        self.set_state(BlockState::Unallocated);
        if space.rc_enabled {
            self.set_as_defrag_source(false);
        }
    }

    pub fn start_line(&self) -> Line {
        Line::from_aligned_address(self.start())
    }

    pub fn end_line(&self) -> Line {
        Line::from_aligned_address(self.end())
    }

    /// Return the `idx`-th line (`0..Block::LINES`) within this block.
    #[inline(always)]
    pub(crate) fn line_at(&self, idx: Fin<LINES_IN_BLOCK>) -> Line {
        self.start_line().next_nth(idx.get())
    }

    /// Get the range of lines within the block.
    #[allow(clippy::assertions_on_constants)]
    pub fn lines(&self) -> RegionIterator<Line> {
        debug_assert!(!super::BLOCK_ONLY);
        RegionIterator::<Line>::new(self.start_line(), self.end_line())
    }

    pub fn clear_rc_table(&self) {
        self.rc_array_checked().clear();
    }

    pub fn clear_striddle_table(&self) {
        crate::util::rc::RC_STRADDLE_LINES.bzero_metadata(self.start(), Block::BYTES);
    }

    #[allow(unused)]
    pub(crate) fn clear_mark_table<VM: VMBinding>(&self) {
        VM::VMObjectModel::LOCAL_MARK_BIT_SPEC
            .extract_side_spec()
            .bzero_metadata(self.start(), Self::BYTES);
    }

    pub(crate) fn initialize_mark_table_as_marked<VM: VMBinding>(&self) {
        VM::VMObjectModel::LOCAL_MARK_BIT_SPEC
            .extract_side_spec()
            .bset_metadata(self.start(), Self::BYTES);
    }

    pub fn log(&self) -> bool {
        loop {
            if Self::TYPED_LOG_TABLE.load_atomic(*self, Ordering::Relaxed) {
                return false;
            }
            if Self::TYPED_LOG_TABLE
                .compare_exchange_atomic(*self, false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return true;
            }
        }
    }

    pub fn set_as_in_place_promoted(&self) {
        if self.is_in_place_promoted() {
            return;
        }
        Self::TYPED_NURSERY_PROMOTION_TABLE.store_atomic(*self, true, Ordering::Relaxed);
    }

    pub fn is_in_place_promoted(&self) -> bool {
        Self::TYPED_NURSERY_PROMOTION_TABLE.load_atomic(*self, Ordering::Relaxed)
    }

    pub fn clear_in_place_promoted(&self) {
        Self::TYPED_NURSERY_PROMOTION_TABLE.store_atomic(*self, false, Ordering::Relaxed);
    }

    pub fn unlog(&self) {
        Self::TYPED_LOG_TABLE.store_atomic(*self, false, Ordering::Relaxed);
    }

    pub fn clear_field_unlog_table<VM: VMBinding>(&self) {
        VM::VMObjectModel::GLOBAL_FIELD_UNLOG_BIT_SPEC
            .as_spec()
            .extract_side_spec()
            .bzero_metadata(self.start(), Block::BYTES);
    }

    pub fn initialize_field_unlog_table_as_unlogged<VM: VMBinding>(&self) {
        VM::VMObjectModel::GLOBAL_FIELD_UNLOG_BIT_SPEC
            .as_spec()
            .extract_side_spec()
            .bset_metadata(self.start(), Block::BYTES);
    }

    /// Sweep this block.
    pub fn sweep<VM: VMBinding>(
        &self,
        space: &ImmixSpace<VM>,
        mark_histogram: &mut Histogram,
        line_mark_state: Option<u8>,
    ) -> BlockSweepResult {
        space
            .mapped_block(*self)
            .expect("Block::sweep requires an allocated block in space")
            .sweep(space, mark_histogram, line_mark_state)
    }

    pub fn rc_sweep_nursery<VM: VMBinding>(&self, space: &ImmixSpace<VM>) -> bool {
        space
            .mapped_block(*self)
            .expect("Block::rc_sweep_nursery requires an allocated block in space")
            .rc_sweep_nursery(space)
    }

    pub fn attempt_mutator_reuse(&self) -> bool {
        self.fetch_update_state(|s| {
            if s.is_reusable() {
                Some(BlockState::Reusing)
            } else {
                None
            }
        })
        .is_ok()
    }

    pub fn rc_sweep_mature<VM: VMBinding>(&self, space: &ImmixSpace<VM>, defrag: bool) -> bool {
        space
            .mapped_block(*self)
            .expect("Block::rc_sweep_mature requires an allocated block in space")
            .rc_sweep_mature::<VM>(space, defrag)
    }

    pub fn rc_table_start(&self) -> Address {
        address_to_meta_address(&crate::util::rc::RC_TABLE, self.start())
    }

    /// Clear VO bits metadata for unmarked regions.
    /// This is useful for clearing VO bits during nursery GC for StickyImmix
    /// at which time young objects (allocated in unmarked regions) may die
    /// but we always consider old objects (in marked regions) as live.
    #[cfg(feature = "vo_bit")]
    pub fn clear_vo_bits_for_unmarked_regions(&self, line_mark_state: Option<u8>) {
        match line_mark_state {
            None => {
                match self.get_state() {
                    BlockState::Unmarked => {
                        // It may contain young objects.  Clear it.
                        vo_bit::bzero_vo_bit(self.start(), Self::BYTES);
                    }
                    BlockState::Marked => {
                        // It contains old objects.  Skip it.
                    }
                    _ => unreachable!(),
                }
            }
            Some(state) => {
                // With lines.
                for line in self.lines() {
                    if !line.is_marked(state) {
                        // It may contain young objects.  Clear it.
                        vo_bit::bzero_vo_bit(line.start(), Line::BYTES);
                    }
                }
            }
        }
    }
}

/// Space-sealed witness that `block` is a non-zero `Block`-aligned Immix block belonging to an
/// allocated chunk of a live `ImmixSpace` whose `SideMetadataContext` maps the space's Immix
/// block/line/RC side-metadata tables across `[block.start(), block.end())`:
/// - `Block::MARK_TABLE` (`IX_BLOCK_MARK`) and `Block::DEFRAG_STATE_TABLE` (`IX_BLOCK_DEFRAG`);
/// - `Line::MARK_TABLE` (`IX_LINE_MARK`) when `!rc_enabled && !super::BLOCK_ONLY`;
/// - `crate::util::rc::RC_TABLE`, `Block::LOG_TABLE` (`IX_BLOCK_LOG`),
///   `Block::NURSERY_PROMOTION_STATE_TABLE`, `RC_STRADDLE_LINES`, and `IX_LINE_REUSE_COUNT`
///   when `rc_enabled`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MappedBlock<'space> {
    block: Block,
    rc_enabled: bool,
    _space: PhantomData<&'space ()>,
}

impl<'space> MappedBlock<'space> {
    /// Construct a `MappedBlock<'space>` after the owning `ImmixSpace` has verified chunk
    /// allocation and mapped side metadata.
    ///
    /// # Safety
    /// * `!block.is_zero()` and `block.start().is_aligned_to(Block::BYTES)`.
    /// * `block.chunk()` must be an allocated chunk of an `ImmixSpace` that lives for `'space`
    ///   with `space.rc_enabled == rc_enabled`, so all side-metadata tables in
    ///   `ImmixSpace::side_metadata_specs(rc_enabled)` (plus global `RC_TABLE` when `rc_enabled`,
    ///   verified in `ImmixSpace::new`) are mapped and initialized for `[block.start(), block.end())`
    ///   for `'space`.
    /// * While any atomic slice returned by `rc_table` or `line_mark_table` is live, no thread may
    ///   perform non-atomic writes or mixed-width atomic accesses on the corresponding metadata
    ///   bytes, and no `&[u8]` / `&mut [u8]` reference may alias them.
    #[inline(always)]
    pub(super) unsafe fn new_unchecked(block: Block, rc_enabled: bool) -> Self {
        debug_assert!(!block.is_zero() && block.start().is_aligned_to(Block::BYTES));
        Self {
            block,
            rc_enabled,
            _space: PhantomData,
        }
    }

    #[inline(always)]
    pub(crate) fn block(&self) -> Block {
        self.block
    }

    /// Project the per-line `RC_TABLE` atomic bytes covering `[block.start(), block.end())`.
    ///
    /// Panics if `!self.rc_enabled`.
    #[inline(always)]
    pub(crate) fn rc_table(&self) -> &'space [[AtomicU8; BYTES_PER_LINE]; LINES_IN_BLOCK] {
        assert!(
            self.rc_enabled,
            "MappedBlock::rc_table requires an RC-enabled ImmixSpace"
        );
        // SAFETY: `MappedBlock<'space>`'s invariant together with `self.rc_enabled` guarantees
        // `!self.block.is_zero()`, `self.block.start()` is aligned to `Block::BYTES` (mapping to
        // `LINES_IN_BLOCK * BYTES_PER_LINE` bytes within a single `BYTES_IN_CHUNK` chunk), global
        // `RC_TABLE` is mapped and initialized for `'space` (checked in `ImmixSpace::new`), and all
        // `RC_TABLE` accesses outside STW `ProcessDecs` use `AtomicU8` without non-atomic or
        // `&mut` aliases.
        unsafe {
            crate::util::rc::RC_TABLE
                .mapped_atomic_byte_chunks_unchecked::<LINES_IN_BLOCK, BYTES_PER_LINE>(
                    self.block.start(),
                )
        }
    }

    /// Project the per-line `IX_LINE_MARK` atomic bytes covering `[block.start(), block.end())`.
    ///
    /// Panics if `self.rc_enabled` or `super::BLOCK_ONLY`.
    #[inline(always)]
    pub(crate) fn line_mark_table(&self) -> &'space [AtomicU8; LINES_IN_BLOCK] {
        assert!(
            !self.rc_enabled && !super::BLOCK_ONLY,
            "MappedBlock::line_mark_table requires a non-RC line-granularity ImmixSpace"
        );
        // SAFETY: `MappedBlock<'space>`'s invariant together with `!self.rc_enabled && !BLOCK_ONLY`
        // guarantees `!self.block.is_zero()`, `self.block.start()` is aligned to `Block::BYTES`
        // (mapping to `LINES_IN_BLOCK` bytes in `Line::MARK_TABLE` within a single chunk),
        // `Line::MARK_TABLE` is mapped and initialized for `'space`, and no non-atomic `bzero`
        // (`acquire_clean_block` on freshly acquired clean blocks) or `&mut [u8]` aliases this
        // allocated block's `Line::MARK_TABLE` bytes while the slice is used.
        unsafe {
            Line::MARK_TABLE.mapped_atomic_bytes_unchecked::<LINES_IN_BLOCK>(self.block.start())
        }
    }

    #[inline(always)]
    pub(super) fn rc_array(&self) -> RCArray<'space> {
        RCArray::from_table(self.block, self.rc_table())
    }

    fn rc_dead(&self) -> bool {
        let rc_array = self.rc_array();
        iter_line_indices().all(|idx| rc_array.is_dead(idx))
    }

    fn has_holes(&self) -> bool {
        let rc_array = self.rc_array();
        let mut found_free_line = false;
        let mut free_lines = 0;
        for idx in iter_line_indices() {
            if rc_array.is_dead(idx) {
                if idx.get() == 0 || found_free_line {
                    free_lines += 1;
                } else {
                    found_free_line = true;
                }
                if free_lines > 0 {
                    return true;
                }
            } else {
                free_lines = 0;
                found_free_line = false;
            }
        }
        false
    }

    pub(crate) fn sweep<VM: VMBinding>(
        &self,
        space: &ImmixSpace<VM>,
        mark_histogram: &mut Histogram,
        line_mark_state: Option<u8>,
    ) -> BlockSweepResult {
        // This method is not called when using RC.
        assert!(!space.rc_enabled && !self.rc_enabled);
        let block = self.block;

        block.set_as_defrag_source(false);
        if super::BLOCK_ONLY {
            match block.get_state() {
                BlockState::Unallocated => unreachable!("Must not sweep unallocated block."),
                BlockState::Unmarked => {
                    #[cfg(feature = "vo_bit")]
                    vo_bit::helper::on_region_swept::<VM, _>(&block, false);

                    // If the pin bit is not on the side, we cannot bulk zero.
                    // We shouldn't need to clear it here in that case, since the pin bit
                    // should be overwritten at each object allocation. The same applies below
                    // when we are sweeping on a line granularity.
                    #[cfg(feature = "object_pinning")]
                    if let MetadataSpec::OnSide(side) = *VM::VMObjectModel::LOCAL_PINNING_BIT_SPEC {
                        side.bzero_metadata(block.start(), Block::BYTES);
                    }

                    // Release the block if it is allocated but not marked by the current GC.
                    space.release_block(block, false);
                    BlockSweepResult::Swept
                }
                BlockState::Marked => {
                    #[cfg(feature = "vo_bit")]
                    vo_bit::helper::on_region_swept::<VM, _>(&block, true);

                    // The block is live.
                    BlockSweepResult::NoReuse
                }
                _ => unreachable!(),
            }
        } else {
            // Calculate number of marked lines and holes.
            let mut marked_lines = 0;
            let mut holes = 0;
            let mut prev_line_is_marked = true;
            let line_mark_state = line_mark_state.unwrap();

            {
                let line_marks = self.line_mark_table();
                for idx in iter_line_indices() {
                    if line_marks[idx].load(Ordering::Relaxed) == line_mark_state {
                        marked_lines += 1;
                        prev_line_is_marked = true;
                    } else {
                        if prev_line_is_marked {
                            holes += 1;
                        }
                        // We need to clear the line mark state at least twice in every 128 GC
                        // otherwise, the line mark state of the last GC will stick around
                        if line_mark_state > Line::MAX_MARK_STATE - 2 {
                            #[cfg(not(feature = "extreme_assertions"))]
                            line_marks[idx].store(0, Ordering::Relaxed);
                            #[cfg(feature = "extreme_assertions")]
                            block.line_at(idx).mark(0);
                        }
                        #[cfg(feature = "immix_zero_on_release")]
                        crate::util::memory::zero(block.line_at(idx).start(), Line::BYTES);

                        // We need to clear the pin bit if it is on the side, as this line can be reused
                        #[cfg(feature = "object_pinning")]
                        if let MetadataSpec::OnSide(side) =
                            *VM::VMObjectModel::LOCAL_PINNING_BIT_SPEC
                        {
                            side.bzero_metadata(block.line_at(idx).start(), Line::BYTES);
                        }

                        prev_line_is_marked = false;
                    }
                }
            }

            if marked_lines == 0 {
                #[cfg(feature = "vo_bit")]
                vo_bit::helper::on_region_swept::<VM, _>(&block, false);

                // Release the block if non of its lines are marked.
                space.release_block(block, false);
                BlockSweepResult::Swept
            } else {
                // There are some marked lines. Keep the block live.
                let is_reusable = marked_lines != Block::LINES;
                if is_reusable {
                    // There are holes. Mark the block as reusable.
                    block.set_state(BlockState::Reusable {
                        unavailable_lines: usize::min(marked_lines, u8::MAX as usize) as _,
                    });
                    space.reusable_blocks.push(block)
                } else {
                    // Clear mark state.
                    block.set_state(BlockState::Unmarked);
                }
                // Update mark_histogram
                mark_histogram[holes] += marked_lines;
                // Record number of holes in block side metadata.
                block.set_holes(holes);

                #[cfg(feature = "vo_bit")]
                vo_bit::helper::on_region_swept::<VM, _>(&block, true);

                if is_reusable {
                    BlockSweepResult::Reused
                } else {
                    BlockSweepResult::NoReuse
                }
            }
        }
    }

    pub(crate) fn rc_sweep_nursery<VM: VMBinding>(&self, space: &ImmixSpace<VM>) -> bool {
        assert!(space.rc_enabled && self.rc_enabled);
        let block = self.block;
        let is_in_place_promoted = block.is_in_place_promoted();
        block.clear_in_place_promoted();
        if is_in_place_promoted {
            block.set_state(BlockState::Reusable {
                unavailable_lines: 1 as _,
            });

            // Bulk clear the VO bits of reusable (unmarked) lines.
            // Lines that are not marked may contain nursery objects that have never received any inc,
            // and their VO bits need to be cleared before the lines can be reused.
            #[cfg(feature = "vo_bit")]
            {
                let rc_array = self.rc_array();
                for idx in iter_line_indices() {
                    if rc_array.is_dead(idx) {
                        let line = block.line_at(idx);
                        crate::util::metadata::vo_bit::bzero_vo_bit(line.start(), Line::BYTES);
                    }
                }
            }

            space.reusable_blocks.push(block);
            false
        } else {
            debug_assert!(self.rc_dead(), "{:?} has non-zero rc value", block);
            debug_assert_ne!(block.get_state(), BlockState::Unallocated);

            // Bulk clear the VO bits of the entire block.
            // This block may contain nursery objects that have never received any inc,
            // and their VO bits need to be cleared before the block can be reused.
            #[cfg(feature = "vo_bit")]
            crate::util::metadata::vo_bit::bzero_vo_bit(block.start(), Block::BYTES);

            space.release_block(block, false);
            true
        }
    }

    pub(crate) fn rc_sweep_mature<VM: VMBinding>(
        &self,
        space: &ImmixSpace<VM>,
        defrag: bool,
    ) -> bool {
        assert!(space.rc_enabled && self.rc_enabled);
        let block = self.block;
        let state = block.get_state();
        if state == BlockState::Unallocated || state == BlockState::Nursery {
            return false;
        }
        if defrag || self.rc_dead() {
            if block.attempt_dealloc(true) {
                // Bulk clear the VO bits of the entire block.
                // Dec operations may reduce some object's RC to 0,
                // at which time their VO bits are cleared, too.
                // But some lines may also contain objects that have never received any inc,
                // and their VO bits need to be cleared before the block can be reused.
                #[cfg(feature = "vo_bit")]
                crate::util::metadata::vo_bit::bzero_vo_bit(block.start(), Block::BYTES);

                space.release_block(block, true);
                return true;
            }
        } else if !super::BLOCK_ONLY {
            // See the caller of this function.
            // At least one object is dead in the block.
            let add_as_reusable = {
                let has_holes = self.has_holes();
                block
                    .fetch_update_state(|s| {
                        if s == BlockState::Reusing
                            || s == BlockState::Unallocated
                            || s.is_reusable()
                            || !has_holes
                        {
                            None
                        } else {
                            Some(BlockState::Reusable {
                                unavailable_lines: 1 as _,
                            })
                        }
                    })
                    .is_ok()
            };
            if add_as_reusable {
                // Bulk clear the VO bits of reusable (unmarked) lines.
                // Dec operations may reduce some object's RC to 0,
                // at which time their VO bits are cleared, too.
                // But some lines may also contain objects that have never received any inc,
                // and their VO bits need to be cleared before the block can be reused.
                #[cfg(feature = "vo_bit")]
                {
                    let rc_array = self.rc_array();
                    for idx in iter_line_indices() {
                        if rc_array.is_dead(idx) {
                            let line = block.line_at(idx);
                            crate::util::metadata::vo_bit::bzero_vo_bit(line.start(), Line::BYTES);
                        }
                    }
                }
                space.reusable_blocks.push(block);
            }
        }
        false
    }

    #[cfg(feature = "vo_bit")]
    pub(crate) fn clear_vo_bits_for_unmarked_regions(&self, line_mark_state: Option<u8>) {
        let block = self.block;
        match line_mark_state {
            None => match block.get_state() {
                BlockState::Unmarked => {
                    vo_bit::bzero_vo_bit(block.start(), Block::BYTES);
                }
                BlockState::Marked => {}
                _ => unreachable!(),
            },
            Some(state) => {
                let line_marks = self.line_mark_table();
                for idx in iter_line_indices() {
                    if line_marks[idx].load(Ordering::Relaxed) != state {
                        let line = block.line_at(idx);
                        vo_bit::bzero_vo_bit(line.start(), Line::BYTES);
                    }
                }
            }
        }
    }
}

/// A non-block single-linked list to store blocks.
pub struct ReusableBlockPool {
    queue: BlockPool<Block>,
    num_workers: usize,
}

impl ReusableBlockPool {
    /// Create empty block list
    pub fn new(num_workers: usize) -> Self {
        Self {
            queue: BlockPool::new(num_workers),
            num_workers,
        }
    }

    /// Get number of blocks in this list.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Add a block to the list.
    pub fn push(&self, block: Block) {
        self.queue.push(block)
    }

    /// Pop a block out of the list.
    pub fn pop(&self) -> Option<Block> {
        self.queue.pop()
    }

    /// Clear the list.
    pub fn reset(&mut self) {
        self.queue = BlockPool::new(self.num_workers);
    }

    /// Iterate all the blocks in the queue. Call the visitor for each reported block.
    pub fn iterate_blocks(&self, mut f: impl FnMut(Block)) {
        self.queue.iterate_blocks(&mut f);
    }

    /// Flush the block queue
    pub fn flush_all(&self) {
        self.queue.flush_all();
    }
}

/// The result of sweeping a block.  Mainly used for statistics.
pub enum BlockSweepResult {
    /// The block is completely free.
    Swept,
    /// The block is partially free, and is reused.
    Reused,
    /// The block cannot be reused.  When [`super::BLOCK_ONLY`] is true, it is returned whenever a
    /// block is not completely free.  Otherwise it is returned when a block is full.
    NoReuse,
}
