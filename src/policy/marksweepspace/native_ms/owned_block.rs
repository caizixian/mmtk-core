//! Verus-style linear ownership typestates (`FreshBlock`, `SweptBlock`, `UnsweptBlock`) for
//! native MarkSweep 64 KiB blocks (`Blueprint 1`).

#![deny(unsafe_op_in_unsafe_fn)]

use crate::util::constants::{BYTES_IN_ADDRESS, MIN_OBJECT_SIZE};
use crate::util::linear_scan::Region;
use crate::util::{Address, VMThread};
use crate::vm::VMBinding;

use super::block_list::{bin_cell_size, BinIndex};
use super::Block;

mod private {
    pub trait Sealed {}
}

/// Sealed trait implemented solely by [`SweptBlock`] and [`UnsweptBlock`] so [`super::BlockList`]
/// can link homogeneous detached blocks intrusively via `PREV_BLOCK_TABLE` and `NEXT_BLOCK_TABLE`
/// while preserving `#[repr(C)]` 1-word null-pointer optimization on `Option<B>`.
pub trait IntrusiveBlock: private::Sealed + Sized {
    /// Return the underlying [`Block`] handle (`0` `unsafe`).
    fn block(&self) -> Block;

    /// Return the cell size (in bytes) recorded in `SIZE_TABLE` for this block.
    #[inline(always)]
    fn cell_size(&self) -> usize {
        self.block().load_block_cell_size()
    }

    /// Reconstruct an owned `Self` from its raw `Block` handle.
    ///
    /// # Safety
    /// Caller must uphold the full representation invariant of `Self` (either as a detached linear
    /// token when returned outside [`super::BlockList`], or as an internal endpoint slot inside
    /// [`super::BlockList`]).
    #[doc(hidden)]
    unsafe fn from_block_unchecked(block: Block) -> Self;
}

/// Linear (`!Copy, !Clone`) ownership of a freshly acquired, mapped 64 KiB `MarkSweepSpace` block
/// before its size-class free-cell chain is initialized.
///
/// # Representation Invariant
/// 1. `self.0` belongs to an allocated `MarkSweepSpace` chunk (`MARK_TABLE == BlockState::Unmarked`
///    and chunk marked allocated in `ChunkMap`) with mapped heap pages `[self.0.start(), self.0.end())`
///    and mapped `Block::METADATA_SPECS` + `LOCAL_MARK_BIT_SPEC` (plus `VO_BIT` and
///    `LOCAL_FREE_LIST_TABLE`/`THREAD_FREE_LIST_TABLE` when enabled).
/// 2. Detached & uniquely owned: `PREV_BLOCK_TABLE == None && NEXT_BLOCK_TABLE == None`, no
///    `BlockList` or other thread holds `FreshBlock`, `SweptBlock`, or `UnsweptBlock` for `self.0`,
///    and no live objects exist in `[self.0.start(), self.0.end())`.
#[repr(transparent)]
pub struct FreshBlock(Block);

static_assertions::assert_eq_size!(FreshBlock, usize);
static_assertions::assert_eq_size!(Option<FreshBlock>, usize);
static_assertions::assert_eq_align!(Option<FreshBlock>, usize);

impl std::fmt::Debug for FreshBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("FreshBlock").field(&self.0).finish()
    }
}

impl FreshBlock {
    /// Mint a `FreshBlock` for a newly acquired and recorded `MarkSweepSpace` block.
    ///
    /// # Safety
    /// Caller must uphold `FreshBlock` representation invariants (1) and (2).
    #[inline(always)]
    pub(super) unsafe fn from_block_unchecked(block: Block) -> Self {
        Self(block)
    }

    /// Return the underlying [`Block`] handle.
    #[inline(always)]
    pub(crate) fn block(&self) -> Block {
        self.0
    }

    /// Consume `self` linearly, initialize its intrusive free-cell chain for MiMalloc size class
    /// `bin` and owning mutator `tls`, and transition into a [`SweptBlock`].
    pub(crate) fn init(self, bin: BinIndex, tls: VMThread) -> SweptBlock {
        let cell_size = bin_cell_size(bin);
        debug_assert!((MIN_OBJECT_SIZE..=Block::BYTES).contains(&cell_size));
        debug_assert_eq!(cell_size % BYTES_IN_ADDRESS, 0);

        let block = self.0;
        let block_end = block.start() + Block::BYTES;
        let mut old_cell = Address::ZERO;
        let mut new_cell = block.start();

        let final_cell = loop {
            // SAFETY: `FreshBlock` invariants (1)-(2) guarantee `[block.start(), block_end)` is
            // mapped and exclusively owned with no live objects, and `bin_cell_size(bin)` is
            // statically verified to be a non-zero multiple of `BYTES_IN_ADDRESS` (`<= Block::BYTES`),
            // so `new_cell` is word-aligned and `[new_cell, new_cell + BYTES_IN_ADDRESS)` lies
            // strictly within `[block.start(), block_end)`.
            unsafe {
                new_cell.store::<Address>(old_cell);
            }
            old_cell = new_cell;
            new_cell += cell_size;
            if new_cell + cell_size > block_end {
                break old_cell;
            }
        };

        // SAFETY: `FreshBlock` invariants (1)-(2) guarantee all per-block side metadata tables are
        // mapped and exclusively owned by `self`, and `final_cell` heads an acyclic `0`-terminated
        // free-cell chain of word-aligned cells of size `cell_size`.
        unsafe {
            block.store_free_list(final_cell);
            block.store_block_cell_size(cell_size);
            #[cfg(feature = "malloc_native_mimalloc")]
            {
                block.store_local_free_list(Address::ZERO);
                block.store_thread_free_list(Address::ZERO);
            }
            block.store_tls(tls);
        }

        SweptBlock(block)
    }
}

/// Linear (`!Copy, !Clone`) ownership of a mapped 64 KiB `MarkSweepSpace` block whose intrusive
/// `FREE_LIST_TABLE` chain is well-formed and ready for mutator cell allocations.
///
/// # Representation Invariant
/// 1. `self.0` belongs to an allocated `MarkSweepSpace` chunk with mapped heap pages
///    `[self.start(), self.end())` and mapped `Block::METADATA_SPECS` + `LOCAL_MARK_BIT_SPEC`
///    (and `VO_BIT` when enabled).
/// 2. When held outside [`super::BlockList`], `self.0` is detached & uniquely owned:
///    `PREV_BLOCK_TABLE == None && NEXT_BLOCK_TABLE == None`, and no `BlockList` or other thread
///    holds `SweptBlock`, `UnsweptBlock`, or `FreshBlock` for `self.0`.
/// 3. `cs = SIZE_TABLE(self.0)` is a valid MiMalloc bin cell size (`MIN_OBJECT_SIZE <= cs <= Block::BYTES`
///    and `cs % BYTES_IN_ADDRESS == 0`), and `FREE_LIST_TABLE(self.0)` heads a finite, acyclic
///    `0`-terminated chain of disjoint word-aligned dead cells
///    `[c_i, c_i + cs) \subseteq [self.start(), self.end())` exclusively owned by the owner of `self`.
#[repr(transparent)]
pub struct SweptBlock(Block);

static_assertions::assert_eq_size!(SweptBlock, usize);
static_assertions::assert_eq_size!(Option<SweptBlock>, usize);
static_assertions::assert_eq_align!(Option<SweptBlock>, usize);

impl private::Sealed for SweptBlock {}

impl IntrusiveBlock for SweptBlock {
    #[inline(always)]
    fn block(&self) -> Block {
        self.0
    }

    #[inline(always)]
    unsafe fn from_block_unchecked(block: Block) -> Self {
        Self(block)
    }
}

impl std::fmt::Debug for SweptBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SweptBlock").field(&self.0).finish()
    }
}

impl SweptBlock {
    /// Return the underlying [`Block`] handle (`0` `unsafe`).
    #[inline(always)]
    pub(crate) fn block(&self) -> Block {
        self.0
    }

    /// Return the block start address (`0` `unsafe`).
    #[inline(always)]
    pub(crate) fn start(&self) -> Address {
        self.0.start()
    }

    /// Return the block end address (`0` `unsafe`).
    #[inline(always)]
    pub(crate) fn end(&self) -> Address {
        self.0.end()
    }

    /// Return the cell size (in bytes) of this block.
    #[inline(always)]
    pub(crate) fn cell_size(&self) -> usize {
        self.0.load_block_cell_size()
    }

    /// Record `tls` as the mutator owning this block.
    #[inline(always)]
    pub(crate) fn set_tls(&mut self, tls: VMThread) {
        // SAFETY: `SweptBlock` invariants (1)-(2) guarantee `TLS_TABLE` is mapped for `self.0`
        // and `&mut self` provides exclusive access.
        unsafe {
            self.0.store_tls(tls);
        }
    }

    /// Return `true` iff this block's intrusive free list has at least one free cell.
    #[inline(always)]
    pub(crate) fn has_free_cells(&self) -> bool {
        // SAFETY: `SweptBlock` invariants (1)-(3) guarantee `FREE_LIST_TABLE` is mapped for
        // `self.0` and no concurrent writes occur while `&self` is borrowed.
        !unsafe { self.0.load_free_list() }.is_zero()
    }

    /// Pop and zero one cell of size `self.cell_size()`, returning `Address::ZERO` if the block
    /// has no free cells remaining.
    #[inline]
    pub(crate) fn alloc_cell(&mut self) -> Address {
        // SAFETY: `SweptBlock` invariants (1)-(3) guarantee `FREE_LIST_TABLE` is mapped and
        // exclusively accessed via `&mut self`.
        let cell = unsafe { self.0.load_free_list() };
        if cell.is_zero() {
            return Address::ZERO;
        }
        // SAFETY: `SweptBlock` invariant (3) guarantees non-zero `cell` is a valid, exclusively
        // owned, word-aligned free cell `[cell, cell + cs)` inside `[self.start(), self.end())`
        // whose first word holds the next intrusive free-list link.
        let next_cell = unsafe { cell.load::<Address>() };
        debug_assert!(
            next_cell.is_zero() || self.0.includes_address(next_cell),
            "next_cell {} is not in {:?}",
            next_cell,
            self.0
        );
        // SAFETY: `FREE_LIST_TABLE` is mapped and exclusively accessed via `&mut self`, and
        // `next_cell` heads the remaining suffix of the acyclic free-cell chain, preserving (3).
        unsafe {
            self.0.store_free_list(next_cell);
        }

        // Zero the cell right before returning it (also clearing the intrusive free-list link
        // previously stored in the first word of `cell`).
        let cell_size = self.0.load_block_cell_size();
        crate::util::memory::zero(cell, cell_size);

        #[cfg(debug_assertions)]
        {
            let mut cursor = cell;
            while cursor < cell + cell_size {
                // SAFETY: `cursor` is word-aligned (`cell` and `cell_size` are word-aligned) and
                // lies within the freshly zeroed, exclusively owned `[cell, cell + cell_size)`.
                debug_assert_eq!(unsafe { cursor.load::<usize>() }, 0);
                cursor += BYTES_IN_ADDRESS;
            }
        }

        cell
    }
}

/// Linear (`!Copy, !Clone`) ownership of a `BlockState::Marked` 64 KiB `MarkSweepSpace` block
/// after STW trace completion, awaiting a single linear [`Self::sweep`] before cell allocation.
///
/// # Representation Invariant
/// 1. `self.0` belongs to an allocated `MarkSweepSpace` chunk with mapped heap pages
///    `[self.0.start(), self.0.end())` and mapped `Block::METADATA_SPECS` + `LOCAL_MARK_BIT_SPEC`
///    (and `VO_BIT` when enabled), with `MARK_TABLE == BlockState::Marked` established by STW tracing.
/// 2. When held outside [`super::BlockList`], `self.0` is detached & uniquely owned:
///    `PREV_BLOCK_TABLE == None && NEXT_BLOCK_TABLE == None`, and no `BlockList`, GC worker, or
///    other mutator concurrently accesses `self.0`'s side metadata or unmarked cells.
/// 3. `cs = SIZE_TABLE(self.0)` is a valid MiMalloc bin cell size (`MIN_OBJECT_SIZE <= cs <= Block::BYTES`
///    and `cs % BYTES_IN_ADDRESS == 0`).
#[repr(transparent)]
pub struct UnsweptBlock(Block);

static_assertions::assert_eq_size!(UnsweptBlock, usize);
static_assertions::assert_eq_size!(Option<UnsweptBlock>, usize);
static_assertions::assert_eq_align!(Option<UnsweptBlock>, usize);

impl private::Sealed for UnsweptBlock {}

impl IntrusiveBlock for UnsweptBlock {
    #[inline(always)]
    fn block(&self) -> Block {
        self.0
    }

    #[inline(always)]
    unsafe fn from_block_unchecked(block: Block) -> Self {
        Self(block)
    }
}

impl std::fmt::Debug for UnsweptBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("UnsweptBlock").field(&self.0).finish()
    }
}

impl UnsweptBlock {
    /// Return the underlying [`Block`] handle (`0` `unsafe`).
    #[inline(always)]
    pub(crate) fn block(&self) -> Block {
        self.0
    }

    /// Return the block start address (`0` `unsafe`).
    #[inline(always)]
    pub(crate) fn start(&self) -> Address {
        self.0.start()
    }

    /// Return the cell size (in bytes) of this block.
    #[inline(always)]
    pub(crate) fn cell_size(&self) -> usize {
        self.0.load_block_cell_size()
    }

    /// Record `tls` as the mutator owning this block.
    #[inline(always)]
    pub(crate) fn set_tls(&mut self, tls: VMThread) {
        // SAFETY: `UnsweptBlock` invariants (1)-(2) guarantee `TLS_TABLE` is mapped for `self.0`
        // and `&mut self` provides exclusive access.
        unsafe {
            self.0.store_tls(tls);
        }
    }

    /// Consume `self` linearly by value (making double-sweep statically impossible) and rebuild
    /// its intrusive free-cell chain from side mark bits, returning a [`SweptBlock`].
    #[inline]
    pub(crate) fn sweep<VM: VMBinding>(self) -> SweptBlock {
        // SAFETY: `UnsweptBlock` invariants (1)-(3) guarantee `self.0` has mapped data pages and
        // local/mark/VO side metadata, a valid word-aligned cell size in `SIZE_TABLE`, and
        // exclusive ownership of unmarked cells and `FREE_LIST_TABLE` outside STW tracing.
        // `Block::sweep` monotonically scans `[self.0.start(), self.0.end())` in steps of `cs`
        // and writes an acyclic `0`-terminated chain of dead cells into `FREE_LIST_TABLE`,
        // establishing `SweptBlock` invariant (3) while preserving (1)-(2).
        unsafe {
            self.0.sweep::<VM>();
        }
        SweptBlock(self.0)
    }
}
