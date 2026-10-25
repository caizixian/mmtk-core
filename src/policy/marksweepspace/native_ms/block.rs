// adapted from Immix
#![deny(unsafe_op_in_unsafe_fn)]

use atomic::Ordering;

use crate::util::constants::LOG_BYTES_IN_PAGE;
use crate::util::heap::chunk_map::*;
use crate::util::linear_scan::Region;
use crate::util::linear_scan::UnstraddlableRegion;
use crate::util::metadata::side_metadata::{MetadataCodec, TypedSideSpec, U8Bits, UsizeBits};
use crate::util::object_enum::BlockMayHaveObjects;
use crate::vm::ObjectModel;
use crate::{
    util::{
        metadata::side_metadata::SideMetadataSpec, Address, AlignedAddress, ObjectReference,
        OpaquePointer, VMThread,
    },
    vm::VMBinding,
};

/// A 64 KiB region for MiMalloc.
///
/// This is also known as a MiMalloc page. We call it `Block` to avoid confusion with OS 4 KiB pages.
/// Backed by [`AlignedAddress<16>`] (`#[repr(transparent)]` over `NonNull<u8>`), guaranteeing:
/// 1. Non-zero 64 KiB alignment (`size_of::<Option<Block>>() == size_of::<usize>()` via NPO), and
/// 2. Zero-`unsafe`, provenance-preserving `Block::start(&self) -> Address` recovery.
#[derive(Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Block(AlignedAddress<16>);

static_assertions::assert_eq_size!(Block, usize);
static_assertions::assert_eq_size!(Option<Block>, usize);
static_assertions::assert_eq_align!(Option<Block>, usize);

impl std::fmt::Debug for Block {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Block(0x{:x})", self.0)
    }
}

impl Region for Block {
    const LOG_BYTES: usize = 16;

    fn from_aligned_address(address: Address) -> Self {
        Self(AlignedAddress::<16>::from_aligned_address(address))
    }

    #[inline(always)]
    fn start(&self) -> Address {
        self.0.to_address()
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

    pub const METADATA_SPECS: [SideMetadataSpec; 6] = [
        Self::MARK_TABLE,
        Self::NEXT_BLOCK_TABLE,
        Self::PREV_BLOCK_TABLE,
        Self::FREE_LIST_TABLE,
        Self::SIZE_TABLE,
        Self::TLS_TABLE,
    ];

    /// Block mark table (side)
    pub const MARK_TABLE: SideMetadataSpec =
        crate::util::metadata::side_metadata::spec_defs::MS_BLOCK_MARK;

    pub const NEXT_BLOCK_TABLE: SideMetadataSpec =
        crate::util::metadata::side_metadata::spec_defs::MS_BLOCK_NEXT;

    pub const PREV_BLOCK_TABLE: SideMetadataSpec =
        crate::util::metadata::side_metadata::spec_defs::MS_BLOCK_PREV;

    pub const FREE_LIST_TABLE: SideMetadataSpec =
        crate::util::metadata::side_metadata::spec_defs::MS_FREE;

    // needed for non GC context
    #[cfg(feature = "malloc_native_mimalloc")]
    pub const LOCAL_FREE_LIST_TABLE: SideMetadataSpec =
        crate::util::metadata::side_metadata::spec_defs::MS_LOCAL_FREE;

    #[cfg(feature = "malloc_native_mimalloc")]
    pub const THREAD_FREE_LIST_TABLE: SideMetadataSpec =
        crate::util::metadata::side_metadata::spec_defs::MS_THREAD_FREE;

    pub const SIZE_TABLE: SideMetadataSpec =
        crate::util::metadata::side_metadata::spec_defs::MS_BLOCK_SIZE;

    pub const TLS_TABLE: SideMetadataSpec =
        crate::util::metadata::side_metadata::spec_defs::MS_BLOCK_TLS;

    // Compile-time region & bit-width validated typed specs.
    const TYPED_MARK_TABLE: TypedSideSpec<Block, U8Bits, BlockState> =
        TypedSideSpec::new(Self::MARK_TABLE);
    const TYPED_NEXT_BLOCK_TABLE: TypedSideSpec<Block, UsizeBits> =
        TypedSideSpec::new(Self::NEXT_BLOCK_TABLE);
    const TYPED_PREV_BLOCK_TABLE: TypedSideSpec<Block, UsizeBits> =
        TypedSideSpec::new(Self::PREV_BLOCK_TABLE);
    const TYPED_FREE_LIST_TABLE: TypedSideSpec<Block, UsizeBits> =
        TypedSideSpec::new(Self::FREE_LIST_TABLE);
    const TYPED_SIZE_TABLE: TypedSideSpec<Block, UsizeBits> = TypedSideSpec::new(Self::SIZE_TABLE);
    const TYPED_TLS_TABLE: TypedSideSpec<Block, UsizeBits> = TypedSideSpec::new(Self::TLS_TABLE);

    /// Load the head cell address of this block's free list.
    ///
    /// # Safety
    /// `self` must belong to an allocated `MarkSweepSpace` chunk with mapped `FREE_LIST_TABLE`
    /// side metadata, and no other thread may concurrently write to `self`'s `FREE_LIST_TABLE`
    /// entry.
    pub unsafe fn load_free_list(&self) -> Address {
        // SAFETY: Caller guarantees `FREE_LIST_TABLE` is mapped for `self` with no concurrent
        // writes; `Address::from_usize` wraps the raw free-list cell address value.
        unsafe { Address::from_usize(Self::TYPED_FREE_LIST_TABLE.load(*self)) }
    }

    /// Store the head cell address of this block's free list.
    ///
    /// # Safety
    /// `self` must belong to an allocated `MarkSweepSpace` chunk with mapped `FREE_LIST_TABLE`
    /// side metadata, and the caller must have exclusive access to `self`'s `FREE_LIST_TABLE`
    /// entry.
    pub unsafe fn store_free_list(&self, free_list: Address) {
        // SAFETY: Caller guarantees `FREE_LIST_TABLE` is mapped for `self` and exclusively
        // accessed by this thread.
        unsafe { Self::TYPED_FREE_LIST_TABLE.store(*self, free_list.as_usize()) }
    }

    /// Load the head cell address of this block's thread-local free list.
    ///
    /// # Safety
    /// `self` must belong to an allocated `MarkSweepSpace` chunk with mapped
    /// `LOCAL_FREE_LIST_TABLE` side metadata, and no other thread may concurrently write to
    /// `self`'s `LOCAL_FREE_LIST_TABLE` entry.
    #[cfg(feature = "malloc_native_mimalloc")]
    #[allow(dead_code)]
    pub unsafe fn load_local_free_list(&self) -> Address {
        // SAFETY: Caller guarantees `LOCAL_FREE_LIST_TABLE` is mapped for `self.start()` with no
        // concurrent writes; `Address::from_usize` wraps the raw free-list address value.
        unsafe { Address::from_usize(Block::LOCAL_FREE_LIST_TABLE.load::<usize>(self.start())) }
    }

    /// Store the head cell address of this block's thread-local free list.
    ///
    /// # Safety
    /// `self` must belong to an allocated `MarkSweepSpace` chunk with mapped
    /// `LOCAL_FREE_LIST_TABLE` side metadata, and the caller must have exclusive access to
    /// `self`'s `LOCAL_FREE_LIST_TABLE` entry.
    #[cfg(feature = "malloc_native_mimalloc")]
    pub unsafe fn store_local_free_list(&self, local_free: Address) {
        // SAFETY: Caller guarantees `LOCAL_FREE_LIST_TABLE` is mapped for `self.start()` and
        // exclusively accessed by this thread.
        unsafe { Block::LOCAL_FREE_LIST_TABLE.store::<usize>(self.start(), local_free.as_usize()) }
    }

    /// Atomically load the head cell address of this block's cross-thread free list.
    ///
    /// # Safety
    /// `self` must belong to an allocated `MarkSweepSpace` chunk with mapped
    /// `THREAD_FREE_LIST_TABLE` side metadata, and any concurrent accesses to `self`'s
    /// `THREAD_FREE_LIST_TABLE` entry must also be atomic.
    #[cfg(feature = "malloc_native_mimalloc")]
    #[allow(dead_code)]
    pub unsafe fn load_thread_free_list(&self) -> Address {
        let raw =
            Block::THREAD_FREE_LIST_TABLE.load_atomic::<usize>(self.start(), Ordering::SeqCst);
        // SAFETY: `Address::from_usize` wraps the loaded raw free-list address value.
        unsafe { Address::from_usize(raw) }
    }

    /// Non-atomically initialize or store the head cell address of this block's cross-thread free list.
    ///
    /// # Safety
    /// `self` must belong to an allocated `MarkSweepSpace` chunk with mapped
    /// `THREAD_FREE_LIST_TABLE` side metadata, and the caller must have exclusive access to
    /// `self`'s `THREAD_FREE_LIST_TABLE` entry (no concurrent reads or writes).
    #[cfg(feature = "malloc_native_mimalloc")]
    pub unsafe fn store_thread_free_list(&self, thread_free: Address) {
        // SAFETY: Caller guarantees `THREAD_FREE_LIST_TABLE` is mapped for `self.start()` and
        // exclusively accessed by this thread.
        unsafe {
            Block::THREAD_FREE_LIST_TABLE.store::<usize>(self.start(), thread_free.as_usize())
        }
    }

    #[cfg(feature = "malloc_native_mimalloc")]
    #[allow(dead_code)]
    fn cas_thread_free_list(&self, old_thread_free: Address, new_thread_free: Address) -> bool {
        Block::THREAD_FREE_LIST_TABLE
            .compare_exchange_atomic::<usize>(
                self.start(),
                old_thread_free.as_usize(),
                new_thread_free.as_usize(),
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
    }

    /// Load the previous `Block` link from `PREV_BLOCK_TABLE`.
    ///
    /// # Safety
    /// `self` must belong to an allocated `MarkSweepSpace` chunk with mapped `PREV_BLOCK_TABLE`
    /// side metadata, no other thread may concurrently write to `self`'s `PREV_BLOCK_TABLE`
    /// entry, and any non-zero entry must be a valid 64 KiB-aligned `Block` start address.
    pub unsafe fn load_prev_block(&self) -> Option<Block> {
        // SAFETY: Caller guarantees `PREV_BLOCK_TABLE` is mapped for `self` with no concurrent
        // writes, and any non-zero value is a valid 64 KiB-aligned `Block` start address.
        let prev_addr = unsafe {
            let prev = Self::TYPED_PREV_BLOCK_TABLE.load(*self);
            if prev == 0 {
                return None;
            }
            Address::from_usize(prev)
        };
        Some(Self::from_aligned_address(prev_addr))
    }

    /// Load the next `Block` link from `NEXT_BLOCK_TABLE`.
    ///
    /// # Safety
    /// `self` must belong to an allocated `MarkSweepSpace` chunk with mapped `NEXT_BLOCK_TABLE`
    /// side metadata, no other thread may concurrently write to `self`'s `NEXT_BLOCK_TABLE`
    /// entry, and any non-zero entry must be a valid 64 KiB-aligned `Block` start address.
    pub unsafe fn load_next_block(&self) -> Option<Block> {
        // SAFETY: Caller guarantees `NEXT_BLOCK_TABLE` is mapped for `self` with no concurrent
        // writes, and any non-zero value is a valid 64 KiB-aligned `Block` start address.
        let next_addr = unsafe {
            let next = Self::TYPED_NEXT_BLOCK_TABLE.load(*self);
            if next == 0 {
                return None;
            }
            Address::from_usize(next)
        };
        Some(Self::from_aligned_address(next_addr))
    }

    /// Store `next` into `self`'s `NEXT_BLOCK_TABLE` link.
    ///
    /// # Safety
    /// `self` must belong to an allocated `MarkSweepSpace` chunk with mapped `NEXT_BLOCK_TABLE`
    /// side metadata, and the caller must have exclusive access to `self`'s `NEXT_BLOCK_TABLE`
    /// entry.
    pub unsafe fn store_next_block(&self, next: Block) {
        // SAFETY: Caller guarantees `NEXT_BLOCK_TABLE` is mapped for `self` and exclusively
        // accessed by this thread.
        unsafe {
            Self::TYPED_NEXT_BLOCK_TABLE.store(*self, next.start().as_usize());
        }
    }

    /// Clear `self`'s `NEXT_BLOCK_TABLE` link to `None` (`0`).
    ///
    /// # Safety
    /// `self` must belong to an allocated `MarkSweepSpace` chunk with mapped `NEXT_BLOCK_TABLE`
    /// side metadata, and the caller must have exclusive access to `self`'s `NEXT_BLOCK_TABLE`
    /// entry.
    pub unsafe fn clear_next_block(&self) {
        // SAFETY: Caller guarantees `NEXT_BLOCK_TABLE` is mapped for `self` and exclusively
        // accessed by this thread.
        unsafe {
            Self::TYPED_NEXT_BLOCK_TABLE.store(*self, 0);
        }
    }

    /// Store `prev` into `self`'s `PREV_BLOCK_TABLE` link.
    ///
    /// # Safety
    /// `self` must belong to an allocated `MarkSweepSpace` chunk with mapped `PREV_BLOCK_TABLE`
    /// side metadata, and the caller must have exclusive access to `self`'s `PREV_BLOCK_TABLE`
    /// entry.
    pub unsafe fn store_prev_block(&self, prev: Block) {
        // SAFETY: Caller guarantees `PREV_BLOCK_TABLE` is mapped for `self` and exclusively
        // accessed by this thread.
        unsafe {
            Self::TYPED_PREV_BLOCK_TABLE.store(*self, prev.start().as_usize());
        }
    }

    /// Clear `self`'s `PREV_BLOCK_TABLE` link to `None` (`0`).
    ///
    /// # Safety
    /// `self` must belong to an allocated `MarkSweepSpace` chunk with mapped `PREV_BLOCK_TABLE`
    /// side metadata, and the caller must have exclusive access to `self`'s `PREV_BLOCK_TABLE`
    /// entry.
    pub unsafe fn clear_prev_block(&self) {
        // SAFETY: Caller guarantees `PREV_BLOCK_TABLE` is mapped for `self` and exclusively
        // accessed by this thread.
        unsafe {
            Self::TYPED_PREV_BLOCK_TABLE.store(*self, 0);
        }
    }

    pub fn load_block_cell_size(&self) -> usize {
        Self::TYPED_SIZE_TABLE.load_atomic(*self, Ordering::SeqCst)
    }

    /// Store the cell size (in bytes) for this block.
    ///
    /// # Safety
    /// `self` must belong to an allocated `MarkSweepSpace` chunk with mapped `SIZE_TABLE` side
    /// metadata, and the caller must have exclusive access to `self`'s `SIZE_TABLE` entry.
    pub unsafe fn store_block_cell_size(&self, size: usize) {
        debug_assert_ne!(size, 0);
        // SAFETY: Caller guarantees `SIZE_TABLE` is mapped for `self` and exclusively accessed by
        // this thread.
        unsafe { Self::TYPED_SIZE_TABLE.store(*self, size) }
    }

    /// Store the owning mutator `VMThread` into `TLS_TABLE`.
    ///
    /// # Safety
    /// `self` must belong to an allocated `MarkSweepSpace` chunk with mapped `TLS_TABLE` side
    /// metadata, and the caller must have exclusive access to `self`'s `TLS_TABLE` entry.
    pub unsafe fn store_tls(&self, tls: VMThread) {
        let tls_usize: usize = tls.0.to_address().as_usize();
        // SAFETY: Caller guarantees `TLS_TABLE` is mapped for `self` and exclusively accessed by
        // this thread.
        unsafe { Self::TYPED_TLS_TABLE.store(*self, tls_usize) }
    }

    /// Load the owning mutator `VMThread` from `TLS_TABLE`.
    ///
    /// # Safety
    /// `self` must belong to an allocated `MarkSweepSpace` chunk with mapped `TLS_TABLE` side
    /// metadata, and any concurrent accesses to `self`'s `TLS_TABLE` entry must be atomic.
    pub unsafe fn load_tls(&self) -> VMThread {
        let tls = Self::TYPED_TLS_TABLE.load_atomic(*self, Ordering::SeqCst);
        // SAFETY: `Address::from_usize` wraps the raw `VMThread` opaque pointer value previously
        // stored via `store_tls`.
        VMThread(OpaquePointer::from_address(unsafe {
            Address::from_usize(tls)
        }))
    }

    /// Return `true` if this block's free list is non-empty.
    ///
    /// # Safety
    /// `self` must belong to an allocated `MarkSweepSpace` chunk with mapped `FREE_LIST_TABLE`
    /// side metadata, and no other thread may concurrently write to `self`'s `FREE_LIST_TABLE`
    /// entry.
    pub unsafe fn has_free_cells(&self) -> bool {
        // SAFETY: Caller guarantees `FREE_LIST_TABLE` is mapped for `self` with no concurrent
        // writes.
        !unsafe { self.load_free_list() }.is_zero()
    }

    /// Get block mark state.
    pub fn get_state(&self) -> BlockState {
        Self::TYPED_MARK_TABLE.load_atomic(*self, Ordering::SeqCst)
    }

    /// Set block mark state.
    pub fn set_state(&self, state: BlockState) {
        Self::TYPED_MARK_TABLE.store_atomic(*self, state, Ordering::SeqCst);
    }

    /// Sweep the block. This is done either lazily in the allocation phase, or eagerly at the end of a GC.
    ///
    /// # Safety
    /// - `self` must be an initialized `MarkSweepSpace` block whose data pages `[self.start(), self.end())`
    ///   and local metadata (`SIZE_TABLE`, `FREE_LIST_TABLE`, `LOCAL_MARK_BIT_SPEC`, and `VO_BIT` if
    ///   enabled) are mapped, with a valid word-aligned non-zero cell size stored in `SIZE_TABLE`.
    /// - The caller must have exclusive access to `self`'s free-list metadata and unmarked cells
    ///   (no mutator or other GC worker may concurrently allocate from, sweep, or access unmarked
    ///   cells in `self`).
    pub unsafe fn sweep<VM: VMBinding>(&self) {
        // The important point here is that we need to distinguish cell address, allocation address, and object reference.
        // We only know cell addresses here. We do not know the allocation address, and we also do not know the object reference.
        // The mark bit is set for object references, and we need to use the mark bit to decide whether a cell is live or not.

        // We haven't implemented for malloc/free cases, for which we do not have mark bit. We could use valid object bit instead.
        if cfg!(feature = "malloc_native_mimalloc") {
            unimplemented!()
        }

        // Check if we can treat it as the simple case: cell address === object reference.
        // If the binding does not use allocation offset, and they use the same allocation alignment which the cell size is aligned to,
        // then we have cell address === allocation address.
        // Furthermore, if the binding does not have an offset between allocation and object reference, then allocation address === cell address.
        if !VM::USE_ALLOCATION_OFFSET
            && VM::MAX_ALIGNMENT == VM::MIN_ALIGNMENT
            && crate::util::conversions::raw_is_aligned(
                self.load_block_cell_size(),
                VM::MAX_ALIGNMENT,
            )
            && VM::VMObjectModel::UNIFIED_OBJECT_REFERENCE_ADDRESS
        {
            // In this case, we can use the simplest and the most efficicent sweep.
            // SAFETY: Caller upholds all `sweep` preconditions, and the `if` condition proves
            // cell start addresses equal object reference addresses for `VM`.
            unsafe { self.simple_sweep::<VM>() }
        } else {
            // Otherwise we fallback to a generic but slow sweep. This roughly has ~10% mutator overhead for lazy sweeping.
            // SAFETY: Caller upholds all `sweep` preconditions for `self`.
            unsafe { self.naive_brute_force_sweep::<VM>() }
        }
    }

    /// This implementation uses object reference and cell address interchangably. This is not correct for most cases.
    /// However, in certain cases, such as OpenJDK, this is correct, and efficient. See the sweep method for the invariants
    /// that we need to use this method correctly.
    ///
    /// # Safety
    /// Same preconditions as [`Self::sweep`], plus `VM` must satisfy cell-address === object-reference
    /// (`!VM::USE_ALLOCATION_OFFSET`, `VM::MAX_ALIGNMENT == VM::MIN_ALIGNMENT`, cell size aligned to
    /// `VM::MAX_ALIGNMENT`, and `VM::VMObjectModel::UNIFIED_OBJECT_REFERENCE_ADDRESS`).
    unsafe fn simple_sweep<VM: VMBinding>(&self) {
        assert!(
            VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.is_on_side(),
            "MarkSweepSpace requires side mark bits"
        );
        let mark_side = VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.extract_side_spec();
        let cell_size = self.load_block_cell_size();
        debug_assert_ne!(cell_size, 0);
        let mut cell = self.start();
        let mut last = Address::ZERO;
        while cell + cell_size <= self.start() + Block::BYTES {
            // The invariants checked in `sweep` ensure cell start addresses equal object reference
            // addresses for `VM`. Querying side mark bits directly by `Address` avoids fabricating
            // an `ObjectReference` for unallocated free-list cells.
            if mark_side.load_atomic::<u8>(cell, Ordering::SeqCst) == 0 {
                // clear VO bit if it is ever set. It is possible that the VO bit is never set for this cell (i.e. there was no object in this cell before this GC),
                // we unset the bit anyway.
                #[cfg(feature = "vo_bit")]
                crate::util::metadata::vo_bit::VO_BIT_SIDE_METADATA_SPEC.store_atomic::<u8>(
                    cell,
                    0,
                    Ordering::SeqCst,
                );
                // SAFETY: `cell` is word-aligned (`self.start()` is 64KB-aligned as enforced by
                // `Block::from_aligned_address`, and `cell_size` is a multiple of `BYTES_IN_ADDRESS`),
                // within mapped block memory `[self.start(), self.end())`, `cell_size >= BYTES_IN_ADDRESS`,
                // and this unmarked cell is dead and exclusively owned by the sweeping thread.
                unsafe {
                    cell.store::<Address>(last);
                }
                last = cell;
            }
            cell += cell_size;
        }

        // SAFETY: Caller guarantees exclusive access to `self`'s mapped `FREE_LIST_TABLE` entry.
        unsafe { self.store_free_list(last) };
    }

    /// This is a naive implementation that is inefficient but should be correct.
    /// In this implementation, we simply go through each possible object
    /// reference and see if it has the mark bit set. If we find mark bit, that means the cell is alive. If we didn't find
    /// the mark bit in the entire cell, it means the cell is dead.
    ///
    /// # Safety
    /// Same preconditions as [`Self::sweep`].
    unsafe fn naive_brute_force_sweep<VM: VMBinding>(&self) {
        use crate::util::constants::MIN_OBJECT_SIZE;
        const _: () = assert!(MIN_OBJECT_SIZE == ObjectReference::ALIGNMENT);

        assert!(
            VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.is_on_side(),
            "MarkSweepSpace requires side mark bits"
        );
        let mark_side = VM::VMObjectModel::LOCAL_MARK_BIT_SPEC.extract_side_spec();

        // Cell size for this block.
        let cell_size = self.load_block_cell_size();
        debug_assert_ne!(cell_size, 0);
        // Current cell
        let mut cell = self.start();
        // Last free cell in the free list
        let mut last = Address::ZERO;
        // Clamp lower-bound offset into `[0, cell_size]` so candidate addresses stay strictly
        // within `[cell, cell + cell_size)` and never underflow/overflow into adjacent blocks or
        // unmapped chunks.
        let lower_offset =
            (VM::VMObjectModel::OBJECT_REF_OFFSET_LOWER_BOUND.max(0) as usize).min(cell_size);

        debug!("Sweep block {:?}, cell size {}", self, cell_size);

        while cell + cell_size <= self.end() {
            let cell_end = cell + cell_size;
            let mut candidate = (cell + lower_offset).align_up(ObjectReference::ALIGNMENT);
            let mut is_live = false;

            while candidate < cell_end {
                trace!(
                    "{:?}: cell = {}, last cell in free list = {}, candidate = {}",
                    self,
                    cell,
                    last,
                    candidate,
                );
                if mark_side.load_atomic::<u8>(candidate, Ordering::SeqCst) != 0 {
                    debug!("{:?} Live cell: {}", self, cell);
                    is_live = true;
                    break;
                }
                candidate += MIN_OBJECT_SIZE;
            }

            if !is_live {
                debug!(
                    "{:?} Free cell: {}, last cell in freelist is {}",
                    self, cell, last
                );

                // Clear VO bit: we don't know where the object reference actually is, so we bulk zero the cell.
                #[cfg(feature = "vo_bit")]
                crate::util::metadata::vo_bit::bzero_vo_bit(cell, cell_size);

                // store the previous cell to make the free list
                debug_assert!(last.is_zero() || (last >= self.start() && last < self.end()));
                // SAFETY: `cell` is word-aligned (`self.start()` is 64KB-aligned as enforced by
                // `Block::from_aligned_address`, and `cell_size` is a multiple of `BYTES_IN_ADDRESS`),
                // within mapped block memory `[self.start(), self.end())`, `cell_size >= BYTES_IN_ADDRESS`,
                // and no mark bit was set anywhere in `[cell, cell + cell_size)`, so the cell is dead and
                // exclusively owned by the sweeping thread.
                unsafe {
                    cell.store::<Address>(last);
                }
                last = cell;
            }

            cell = cell_end;
        }

        // SAFETY: Caller guarantees exclusive access to `self`'s mapped `FREE_LIST_TABLE` entry.
        unsafe { self.store_free_list(last) };
    }

    /// Get the chunk containing the block.
    pub fn chunk(&self) -> Chunk {
        Chunk::from_unaligned_address(self.start())
    }

    /// Initialize a clean block after acquired from page-resource.
    pub fn init(&self) {
        self.set_state(BlockState::Unmarked);
    }

    /// Deinitalize a block before releasing.
    pub fn deinit(&self) {
        self.set_state(BlockState::Unallocated);
    }
}

/// The block allocation state.
#[repr(u8)]
#[derive(Debug, PartialEq, Eq, Clone, Copy, bytemuck::NoUninit, bytemuck::CheckedBitPattern)]
pub enum BlockState {
    /// the block is not allocated.
    Unallocated = 0,
    /// the block is allocated but not marked (`u8::MAX`).
    Unmarked = 0xFF,
    /// the block is allocated and marked (`u8::MAX - 1`).
    Marked = 0xFE,
}

const _: () = {
    assert!(BlockState::Unmarked as u8 == u8::MAX);
    assert!(BlockState::Marked as u8 == u8::MAX - 1);
};

impl MetadataCodec<u8> for BlockState {
    #[inline(always)]
    fn encode(self) -> u8 {
        bytemuck::cast(self)
    }

    #[inline(always)]
    fn decode(raw: u8) -> Self {
        bytemuck::checked::try_cast(raw)
            .unwrap_or_else(|_| panic!("Invalid native_ms::BlockState discriminant: {raw:#x}"))
    }
}

impl From<u8> for BlockState {
    #[inline(always)]
    fn from(state: u8) -> Self {
        <Self as MetadataCodec<u8>>::decode(state)
    }
}

impl From<BlockState> for u8 {
    #[inline(always)]
    fn from(state: BlockState) -> Self {
        <BlockState as MetadataCodec<u8>>::encode(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_aligned_address_round_trip_and_npo() {
        #[repr(align(65536))]
        struct MockBlockBuf([u64; Block::BYTES / 8]);

        let mut buf = Box::new(MockBlockBuf([0u64; Block::BYTES / 8]));
        let base = Address::from_mut_ptr(buf.0.as_mut_ptr());
        let block = Block::from_aligned_address(base);
        assert_eq!(block.start(), base);
        assert_eq!(block.end(), base + Block::BYTES);
        assert_eq!(Some(block).map(|b| b.start()), Some(base));

        // Verify provenance is preserved across `Address -> Block -> Address`.
        let recovered = block.start();
        // SAFETY: `recovered` points to `buf.0[0]`, aligned to `u64`, writable, and unaliased.
        unsafe {
            recovered.store::<u64>(0xcafe_babe_dead_beef);
        }
        assert_eq!(buf.0[0], 0xcafe_babe_dead_beef);
    }

    #[test]
    fn test_block_state_bytemuck_codec() {
        for state in [
            BlockState::Unallocated,
            BlockState::Unmarked,
            BlockState::Marked,
        ] {
            let raw = u8::from(state);
            assert_eq!(BlockState::from(raw), state);
            assert_eq!(<BlockState as MetadataCodec<u8>>::decode(raw), state);
        }
        assert_eq!(u8::from(BlockState::Unallocated), 0);
        assert!(bytemuck::checked::try_cast::<u8, BlockState>(1).is_err());
    }

    #[test]
    #[should_panic(expected = "Invalid native_ms::BlockState discriminant")]
    fn test_block_state_invalid_discriminant_panics() {
        let _ = BlockState::from(1u8);
    }
}
