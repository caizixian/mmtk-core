#![deny(unsafe_op_in_unsafe_fn)]

use super::freelist::*;
use crate::util::address::Address;
use crate::util::constants::*;
use crate::util::conversions;
use crate::util::os::*;

/** log2 of the number of bits used by a free list entry (two entries per unit) */
const LOG_ENTRY_BITS: usize = i32::BITS.ilog2() as _;

/** log2 of the number of bytes used by a free list entry (two entries per unit) */
const LOG_BYTES_IN_ENTRY: usize = LOG_ENTRY_BITS - (LOG_BITS_IN_BYTE as usize);

/** log2 of the number of bytes used by a free list unit */
const LOG_BYTES_IN_UNIT: usize = LOG_BYTES_IN_ENTRY + 1;

#[derive(Debug)]
pub struct RawMemoryFreeList {
    pub head: i32,
    pub heads: i32,
    base: Address,
    limit: Address,
    high_water: Address,
    max_units: i32,
    grain: i32,
    current_units: i32,
    pages_per_block: i32,
    strategy: MmapStrategy,
    slice: &'static mut [i32],
}

impl FreeList for RawMemoryFreeList {
    fn head(&self) -> i32 {
        self.head
    }
    fn heads(&self) -> i32 {
        self.heads
    }
    fn get_entry(&self, index: i32) -> i32 {
        #[cfg(debug_assertions)]
        {
            let len = (self.high_water - self.base) >> LOG_BYTES_IN_ENTRY;
            debug_assert_eq!(
                len,
                self.slice.len(),
                "Length does not match.  \
                high_water: {h}, base: {b}, computed len: {len}, \
                slice len: {sl}",
                h = self.high_water,
                b = self.base,
                sl = self.slice.len()
            );
        }
        self.slice[index as usize]
    }
    fn set_entry(&mut self, index: i32, value: i32) {
        #[cfg(debug_assertions)]
        {
            let len = (self.high_water - self.base) >> LOG_BYTES_IN_ENTRY;
            debug_assert_eq!(
                len,
                self.slice.len(),
                "Length does not match.  \
                high_water: {h}, base: {b}, computed len: {len}, \
                slice len: {sl}",
                h = self.high_water,
                b = self.base,
                sl = self.slice.len()
            );
        }
        self.slice[index as usize] = value;
    }
    fn alloc(&mut self, size: i32) -> i32 {
        if self.current_units == 0 {
            return FAILURE;
        }
        let mut unit = self.head();
        let mut s = 0;
        while ({
            unit = self.get_next(unit);
            unit != self.head()
        }) && ({
            s = self.get_size(unit);
            s < size
        }) {}
        if unit == self.head() {
            FAILURE
        } else {
            self.__alloc(size, unit, s)
        }
    }
}

impl RawMemoryFreeList {
    fn units_per_block(&self) -> i32 {
        (conversions::pages_to_bytes(self.pages_per_block as _) >> LOG_BYTES_IN_UNIT) as _
    }
    fn units_in_first_block(&self) -> i32 {
        self.units_per_block() - self.heads - 1
    }
    pub fn default_block_size(units: i32, heads: i32) -> i32 {
        usize::min(Self::size_in_pages(units, heads) as _, 16) as _
    }
    pub fn size_in_pages(units: i32, heads: i32) -> i32 {
        assert!(
            (0..=MAX_UNITS).contains(&units) && (0..=MAX_HEADS).contains(&heads),
            "RawMemoryFreeList units ({units}) or heads ({heads}) out of range"
        );
        let total_units = (units as usize) + (heads as usize) + 1;
        let map_size = total_units
            .checked_shl(LOG_BYTES_IN_UNIT as u32)
            .expect("RawMemoryFreeList byte size overflow");
        i32::try_from(conversions::bytes_to_pages_up(map_size))
            .expect("RawMemoryFreeList page count overflow")
    }

    /// Construct a `RawMemoryFreeList` backed by its own dedicated quarantined anonymous virtual
    /// address range (`PROT_NONE` until pages are demand-zero committed by [`Self::grow_freelist`]).
    pub fn new(
        pages_per_block: i32,
        units: i32,
        grain: i32,
        heads: i32,
        strategy: MmapStrategy,
    ) -> Self {
        assert!(
            matches!(
                strategy.prot,
                MmapProtection::ReadWrite | MmapProtection::ReadWriteExec
            ),
            "RawMemoryFreeList requires readable and writable MmapProtection, got {:?}",
            strategy.prot
        );
        assert!(
            pages_per_block > 0,
            "RawMemoryFreeList pages_per_block ({pages_per_block}) must be > 0"
        );
        assert!(
            (0..=MAX_UNITS).contains(&units),
            "RawMemoryFreeList units ({units}) must be in 0..={MAX_UNITS}"
        );
        assert!(grain > 0, "RawMemoryFreeList grain ({grain}) must be > 0");
        assert!(
            (0..=MAX_HEADS).contains(&heads),
            "RawMemoryFreeList heads ({heads}) must be in 0..={MAX_HEADS}"
        );
        let required_bytes =
            conversions::pages_to_bytes(Self::size_in_pages(units, heads) as usize);
        assert!(
            required_bytes > 0 && required_bytes <= isize::MAX as usize,
            "RawMemoryFreeList required_bytes ({required_bytes}) out of bounds"
        );
        let base = OS::dzmmap_anywhere(
            required_bytes,
            BYTES_IN_PAGE,
            MmapStrategy::QUARANTINE,
            &MmapAnnotation::Misc {
                name: "RawMemoryFreeList",
            },
        )
        .unwrap_or_else(|e| {
            panic!("Failed to quarantine {required_bytes} bytes for RawMemoryFreeList: {e:?}")
        });
        assert!(
            !base.is_zero() && base.is_aligned_to(BYTES_IN_PAGE),
            "OS::dzmmap_anywhere returned invalid base ({base}) for RawMemoryFreeList"
        );
        let limit = base + required_bytes;
        Self {
            head: -1,
            heads,
            base,
            limit,
            high_water: base,
            max_units: units,
            grain,
            current_units: 0,
            pages_per_block,
            strategy: strategy.replace(true),
            slice: &mut [],
        }
    }

    fn current_capacity(&self) -> i32 {
        let list_blocks = conversions::bytes_to_pages_up(self.high_water - self.base) as i32
            / self.pages_per_block;
        self.units_in_first_block() + (list_blocks - 1) * self.units_per_block()
    }

    pub fn grow_freelist(&mut self, units: i32) -> bool {
        assert!(
            units >= 0,
            "RawMemoryFreeList::grow_freelist units ({units}) must be >= 0"
        );
        let Some(required_units) = self.current_units.checked_add(units) else {
            return false;
        };
        if required_units > self.max_units {
            return false;
        }
        let blocks = if required_units > self.current_capacity() {
            let units_requested = required_units - self.current_capacity();
            (units_requested + self.units_per_block() - 1) / self.units_per_block()
        } else {
            0
        };
        self.grow_list_by_blocks(blocks, required_units);
        true
    }

    fn grow_list_by_blocks(&mut self, blocks: i32, new_max: i32) {
        debug_assert!(
            (new_max <= self.grain) || (((new_max / self.grain) * self.grain) == new_max)
        );

        // Drop the old slice reference before growing the mapping and recreating the slice.
        self.slice = &mut [];

        if blocks > 0 {
            self.raise_high_water(blocks);
        }

        let byte_len = self.high_water - self.base;
        assert!(
            byte_len <= isize::MAX as usize,
            "RawMemoryFreeList mapping exceeds isize::MAX bytes"
        );
        let len = byte_len >> LOG_BYTES_IN_ENTRY;
        // SAFETY:
        // - `[self.base, self.limit)` is a dedicated, private anonymous virtual reservation
        //   obtained from `OS::dzmmap_anywhere` in `RawMemoryFreeList::new` and unmapped only in
        //   `Drop` (after resetting `self.slice = &mut []`), so no other Rust reference or mapping
        //   can alias `[self.base, self.high_water)`.
        // - `self.base` was asserted non-zero and `BYTES_IN_PAGE`-aligned (`>= 4096`), so
        //   `self.base.to_mut_ptr::<i32>()` is non-null and aligned to `align_of::<i32>()` (4B).
        // - Every page in `self.base..self.high_water` has been demand-zero committed via
        //   `OS::dzmmap` in `raise_high_water` with readable+writable protection (verified in
        //   `new`), so `len` contiguous `i32` elements (`byte_len <= isize::MAX as usize`) are
        //   mapped, initialized to `0i32`, and exclusively owned by `self`.
        self.slice = unsafe { std::slice::from_raw_parts_mut(self.base.to_mut_ptr::<i32>(), len) };

        let old_max = self.current_units;
        assert!(
            new_max <= self.current_capacity(),
            "blocks and new max are inconsistent: need more blocks for the requested capacity"
        );
        assert!(
            new_max <= self.max_units,
            "Requested list to grow larger than the configured maximum"
        );
        self.current_units = new_max;

        if old_max == 0 {
            // First allocation of capacity: initialize the sentinels.
            for i in 1..=self.heads {
                self.set_sentinel(-i);
            }
        } else {
            // Turn the old top-of-heap sentinel into a single used block
            self.set_size(old_max, 1);
        }

        if new_max == 0 {
            return;
        }

        // Set a sentinel at the top of the new range
        self.set_sentinel(new_max);

        let mut cursor = new_max;

        /* A series of grain size regions in the middle */
        let grain = i32::min(self.grain, new_max - old_max);
        cursor -= grain;
        while cursor >= old_max {
            self.set_size(cursor, grain);
            self.add_to_free(cursor);
            cursor -= grain;
        }
    }

    fn raise_high_water(&mut self, blocks: i32) {
        assert!(
            blocks > 0,
            "RawMemoryFreeList::raise_high_water blocks ({blocks}) must be > 0"
        );
        let block_pages = (self.pages_per_block as usize)
            .checked_mul(blocks as usize)
            .expect("RawMemoryFreeList block page count overflow");
        let mut grow_extent = block_pages
            .checked_mul(BYTES_IN_PAGE)
            .expect("RawMemoryFreeList grow_extent byte overflow");
        assert_ne!(
            self.high_water, self.limit,
            "Attempt to grow FreeList beyond limit"
        );
        if grow_extent > self.limit - self.high_water {
            grow_extent = self.limit - self.high_water;
        }
        let res = OS::dzmmap(
            self.high_water,
            grow_extent,
            self.strategy,
            &MmapAnnotation::Misc {
                name: "RawMemoryFreeList",
            },
        );
        assert!(
            res.is_ok(),
            "Failed to mmap memory for RawMemoryFreeList: start = {}, bytes = {grow_extent}, strategy = {:?}",
            self.high_water,
            self.strategy
        );
        self.high_water += grow_extent;
    }
}

impl Drop for RawMemoryFreeList {
    fn drop(&mut self) {
        self.slice = &mut [];
        let len = self.limit - self.base;
        if len != 0 {
            let _ = OS::munmap(self.base, len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FreeList;
    use super::*;

    const TOP_SENTINEL: i32 = -1;
    const FIRST_UNIT: i32 = 0;

    fn new_raw_memory_freelist(list_size: usize, grain: i32) -> (RawMemoryFreeList, i32, i32) {
        let pages_per_block = RawMemoryFreeList::default_block_size(list_size as _, 1);
        assert_eq!(pages_per_block, 1);
        let mut l = RawMemoryFreeList::new(
            pages_per_block,
            list_size as _,
            grain,
            1,
            MmapStrategy::TEST,
        );
        l.grow_freelist(list_size as _);
        let last_unit = list_size as i32 - grain;
        let bottom_sentinel = list_size as i32;
        (l, last_unit, bottom_sentinel)
    }

    #[test]
    #[allow(clippy::cognitive_complexity)]
    fn new_free_list_grain1() {
        let (l, last_unit, bottom_sentinel) = new_raw_memory_freelist(5, 1);
        assert_eq!(l.head(), TOP_SENTINEL);

        assert_eq!(l.get_prev(TOP_SENTINEL), last_unit);
        assert_eq!(l.get_next(TOP_SENTINEL), FIRST_UNIT);

        assert_eq!(l.get_size(FIRST_UNIT), 1);
        assert_eq!(l.get_left(FIRST_UNIT), -1);
        assert_eq!(l.get_prev(FIRST_UNIT), -1);
        assert_eq!(l.get_right(FIRST_UNIT), 1);
        assert_eq!(l.get_next(FIRST_UNIT), 1);
        assert!(l.is_free(FIRST_UNIT));
        assert!(l.is_coalescable(FIRST_UNIT));
        assert!(!l.is_multi(FIRST_UNIT));

        assert_eq!(l.get_size(1), 1);
        assert_eq!(l.get_left(1), 0);
        assert_eq!(l.get_prev(1), 0);
        assert_eq!(l.get_right(1), 2);
        assert_eq!(l.get_next(1), 2);
        assert!(l.is_free(1));
        assert!(l.is_coalescable(1));
        assert!(!l.is_multi(1));

        assert_eq!(l.get_size(last_unit), 1);
        assert_eq!(l.get_left(last_unit), last_unit - 1);
        assert_eq!(l.get_prev(last_unit), last_unit - 1);
        assert_eq!(l.get_right(last_unit), bottom_sentinel);
        assert_eq!(l.get_next(last_unit), -1);
        assert!(l.is_free(last_unit));
        assert!(l.is_coalescable(last_unit));
        assert!(!l.is_multi(last_unit));

        assert_eq!(l.get_prev(bottom_sentinel), bottom_sentinel);
        assert_eq!(l.get_next(bottom_sentinel), bottom_sentinel);
    }

    #[test]
    #[allow(clippy::cognitive_complexity)]
    fn new_free_list_grain2() {
        let (l, last_unit, bottom_sentinel) = new_raw_memory_freelist(6, 2);
        assert_eq!(l.head(), TOP_SENTINEL);

        assert_eq!(l.get_prev(TOP_SENTINEL), last_unit);
        assert_eq!(l.get_next(TOP_SENTINEL), FIRST_UNIT);

        assert_eq!(l.get_size(FIRST_UNIT), 2);
        assert_eq!(l.get_left(FIRST_UNIT), -1);
        assert_eq!(l.get_prev(FIRST_UNIT), -1);
        assert_eq!(l.get_right(FIRST_UNIT), 2);
        assert_eq!(l.get_next(FIRST_UNIT), 2);
        assert!(l.is_free(FIRST_UNIT));
        assert!(l.is_coalescable(FIRST_UNIT));
        assert!(l.is_multi(FIRST_UNIT));

        assert_eq!(l.get_size(2), 2);
        assert_eq!(l.get_left(2), 0);
        assert_eq!(l.get_prev(2), 0);
        assert_eq!(l.get_right(2), 4);
        assert_eq!(l.get_next(2), 4);
        assert!(l.is_free(2));
        assert!(l.is_coalescable(2));
        assert!(l.is_multi(2));

        assert_eq!(l.get_size(last_unit), 2);
        assert_eq!(l.get_left(last_unit), last_unit - 2);
        assert_eq!(l.get_prev(last_unit), last_unit - 2);
        assert_eq!(l.get_right(last_unit), bottom_sentinel);
        assert_eq!(l.get_next(last_unit), -1);
        assert!(l.is_free(last_unit));
        assert!(l.is_coalescable(last_unit));
        assert!(l.is_multi(last_unit));

        assert_eq!(l.get_prev(bottom_sentinel), bottom_sentinel);
        assert_eq!(l.get_next(bottom_sentinel), bottom_sentinel);
    }

    #[test]
    #[should_panic]
    fn free_list_access_out_of_bounds() {
        let (l, _, _) = new_raw_memory_freelist(5, 1);
        l.get_size(4096);
    }

    #[test]
    fn alloc_fit() {
        let (mut l, last_unit, _) = new_raw_memory_freelist(6, 2);
        let result = l.alloc(2);
        assert_eq!(result, 0);

        const NEXT: i32 = 2;

        assert_eq!(l.get_prev(TOP_SENTINEL), last_unit);
        assert_eq!(l.get_next(TOP_SENTINEL), NEXT);

        assert_eq!(l.get_size(FIRST_UNIT), 2);
        assert_eq!(l.get_left(FIRST_UNIT), -1);
        assert_eq!(l.get_prev(FIRST_UNIT), -1);
        assert_eq!(l.get_right(FIRST_UNIT), 2);
        assert_eq!(l.get_next(FIRST_UNIT), 2);
        assert!(!l.is_free(FIRST_UNIT));
        assert!(l.is_coalescable(FIRST_UNIT));
        assert!(l.is_multi(FIRST_UNIT));

        assert_eq!(l.get_size(2), 2);
        assert_eq!(l.get_left(2), 0);
        assert_eq!(l.get_prev(2), -1);
        assert_eq!(l.get_right(2), 4);
        assert_eq!(l.get_next(2), 4);
        assert!(l.is_free(2));
        assert!(l.is_coalescable(2));
        assert!(l.is_multi(2));
    }

    #[test]
    #[allow(clippy::cognitive_complexity)]
    fn alloc_split() {
        let (mut l, last_unit, _) = new_raw_memory_freelist(6, 2);
        let result = l.alloc(1);
        assert_eq!(result, 0);

        const NEXT: i32 = 1;
        assert_eq!(l.get_prev(TOP_SENTINEL), last_unit);
        assert_eq!(l.get_next(TOP_SENTINEL), NEXT);

        assert_eq!(l.get_size(FIRST_UNIT), 1);
        assert_eq!(l.get_left(FIRST_UNIT), -1);
        assert_eq!(l.get_prev(FIRST_UNIT), 1);
        assert_eq!(l.get_right(FIRST_UNIT), 1);
        assert_eq!(l.get_next(FIRST_UNIT), 2);
        assert!(!l.is_free(FIRST_UNIT));
        assert!(l.is_coalescable(FIRST_UNIT));
        assert!(!l.is_multi(FIRST_UNIT));

        assert_eq!(l.get_size(1), 1);
        assert_eq!(l.get_left(1), 0);
        assert_eq!(l.get_prev(1), -1);
        assert_eq!(l.get_right(1), 2);
        assert_eq!(l.get_next(1), 2);
        assert!(l.is_free(1));
        assert!(l.is_coalescable(1));
        assert!(!l.is_multi(1));

        assert_eq!(l.get_size(2), 2);
        assert_eq!(l.get_left(2), 1);
        assert_eq!(l.get_prev(2), 1);
        assert_eq!(l.get_right(2), 4);
        assert_eq!(l.get_next(2), 4);
        assert!(l.is_free(2));
        assert!(l.is_coalescable(2));
        assert!(l.is_multi(2));
    }

    #[test]
    fn alloc_split_twice() {
        let (mut l, _, _) = new_raw_memory_freelist(6, 2);
        let res1 = l.alloc(1);
        assert_eq!(res1, 0);
        let res2 = l.alloc(1);
        assert_eq!(res2, 1);
        assert_eq!(l.get_prev(2), -1);
    }

    #[test]
    fn alloc_skip() {
        let (mut l, _, _) = new_raw_memory_freelist(6, 2);
        let res1 = l.alloc(1);
        assert_eq!(res1, 0);
        let res2 = l.alloc(2);
        assert_eq!(res2, 2);
        assert!(l.is_free(1));
        assert_eq!(l.get_next(1), 4);
        assert_eq!(l.get_prev(4), 1);
    }

    #[test]
    fn alloc_exhaust() {
        let (mut l, _, _) = new_raw_memory_freelist(6, 2);
        let res1 = l.alloc(2);
        assert_eq!(res1, 0);
        let res2 = l.alloc(2);
        assert_eq!(res2, 2);
        let res3 = l.alloc(2);
        assert_eq!(res3, 4);
        let res4 = l.alloc(2);
        assert_eq!(res4, FAILURE);
    }

    #[test]
    fn free_unit() {
        let (mut l, _, _) = new_raw_memory_freelist(6, 2);
        let res1 = l.alloc(2);
        assert_eq!(res1, 0);
        let res2 = l.alloc(2);
        assert_eq!(res2, 2);
        assert_eq!(l.get_prev(4), -1);
        let freed = l.free(res2, false);
        assert_eq!(freed, res2);
        assert!(l.is_free(res2));
    }

    #[test]
    fn free_coalesce() {
        let (mut l, _, _) = new_raw_memory_freelist(6, 2);
        let res1 = l.alloc(2);
        assert_eq!(res1, 0);
        let res2 = l.alloc(2);
        assert_eq!(res2, 2);
        let coalesced_size = l.free(res2, true);
        assert_eq!(coalesced_size, 4);
    }

    #[test]
    fn free_cant_coalesce() {
        let (mut l, _, _) = new_raw_memory_freelist(6, 2);
        let res1 = l.alloc(2);
        assert_eq!(res1, 0);
        let res2 = l.alloc(2);
        assert_eq!(res2, 2);
        let res3 = l.alloc(1);
        assert_eq!(res3, 4);
        let coalesced_size = l.free(res2, true);
        assert_eq!(coalesced_size, 2);
    }

    #[test]
    fn free_realloc() {
        let (mut l, _, _) = new_raw_memory_freelist(6, 2);
        let res1 = l.alloc(2);
        assert_eq!(res1, 0);
        let res2 = l.alloc(2);
        assert_eq!(res2, 2);
        assert_eq!(l.get_prev(4), -1);
        let freed = l.free(res2, false);
        assert_eq!(freed, res2);
        assert!(l.is_free(res2));
        let res3 = l.alloc(2);
        assert_eq!(res3, 2);
        assert!(!l.is_free(res3));
        let res4 = l.alloc(1);
        assert_eq!(res4, 4);
    }
}
