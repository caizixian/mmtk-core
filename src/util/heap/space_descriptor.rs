use bytemuck::Zeroable;

use crate::util::heap::layout::vm_layout::{self, vm_layout};
use crate::util::Address;
use std::sync::atomic::{AtomicUsize, Ordering};

const TYPE_BITS: usize = 2;
#[allow(unused)]
const TYPE_SHARED: usize = 0;
const TYPE_CONTIGUOUS: usize = 1;
const TYPE_CONTIGUOUS_HI: usize = 3;
const TYPE_MASK: usize = (1 << TYPE_BITS) - 1;
const SIZE_SHIFT: usize = TYPE_BITS;
const SIZE_BITS: usize = 10;
const SIZE_MASK: usize = ((1 << SIZE_BITS) - 1) << SIZE_SHIFT;
const EXPONENT_SHIFT: usize = SIZE_SHIFT + SIZE_BITS;
const EXPONENT_BITS: usize = 5;
const EXPONENT_MASK: usize = ((1 << EXPONENT_BITS) - 1) << EXPONENT_SHIFT;
const MANTISSA_SHIFT: usize = EXPONENT_SHIFT + EXPONENT_BITS;
const MANTISSA_BITS: usize = 14;
// FIXME: The constant `i32::BITS` is inherited from JikesRVM. It looks like the current
// `SpaceDescriptor` encoding can only encode 32-bit address spaces where each space can only have
// less than 1024 (`1 << SIZE_BITS`) chunks. In this case, `BASE_EXPONENT`` will depend on
// `i32::BITS`. If we need to extend `SpaceDescriptor` to 64-bit address spaces, we need to redesign
// the encoding.
const BASE_EXPONENT: usize = i32::BITS as usize - MANTISSA_BITS;

const INDEX_MASK: usize = !TYPE_MASK;
const INDEX_SHIFT: usize = TYPE_BITS;

static DISCONTIGUOUS_SPACE_INDEX: AtomicUsize = AtomicUsize::new(DISCONTIG_INDEX_INCREMENT);
const DISCONTIG_INDEX_INCREMENT: usize = 1 << TYPE_BITS;

#[derive(Copy, Clone, PartialEq, Debug, Zeroable)]
#[repr(transparent)]
pub struct SpaceDescriptor(usize);

/// An atomic wrapper around [`SpaceDescriptor`] with the same layout and bit-validity as `usize`.
#[repr(transparent)]
pub(crate) struct AtomicSpaceDescriptor(AtomicUsize);

// SAFETY: `AtomicSpaceDescriptor` is `#[repr(transparent)]` over `AtomicUsize`, which
// `core::sync::atomic` guarantees has the same size, alignment, and bit-validity as `usize`.
// Every bit pattern (including all-zero bytes, representing `SpaceDescriptor::UNINITIALIZED`)
// is therefore a valid `AtomicSpaceDescriptor`.
unsafe impl Zeroable for AtomicSpaceDescriptor {}

impl AtomicSpaceDescriptor {
    pub(crate) const fn new(descriptor: SpaceDescriptor) -> Self {
        Self(AtomicUsize::new(descriptor.0))
    }

    #[inline]
    pub(crate) fn load(&self, order: Ordering) -> SpaceDescriptor {
        SpaceDescriptor(self.0.load(order))
    }

    #[inline]
    pub(crate) fn store(&self, descriptor: SpaceDescriptor, order: Ordering) {
        self.0.store(descriptor.0, order);
    }

    #[inline]
    pub(crate) fn compare_exchange(
        &self,
        current: SpaceDescriptor,
        new: SpaceDescriptor,
        success: Ordering,
        failure: Ordering,
    ) -> Result<SpaceDescriptor, SpaceDescriptor> {
        self.0
            .compare_exchange(current.0, new.0, success, failure)
            .map(SpaceDescriptor)
            .map_err(SpaceDescriptor)
    }
}

impl SpaceDescriptor {
    pub const UNINITIALIZED: Self = SpaceDescriptor(0);

    pub fn create_descriptor_from_heap_range(start: Address, end: Address) -> SpaceDescriptor {
        let top = end == vm_layout().heap_end;
        if vm_layout().force_use_contiguous_spaces {
            let space_index = if start > vm_layout().heap_end {
                usize::MAX
            } else {
                start >> vm_layout().space_shift_64()
            };
            let flags = if top {
                TYPE_CONTIGUOUS_HI
            } else {
                TYPE_CONTIGUOUS
            };
            return SpaceDescriptor((space_index << INDEX_SHIFT) | flags);
        }
        let chunks = (end - start) >> vm_layout::LOG_BYTES_IN_CHUNK;
        debug_assert!(!start.is_zero() && chunks > 0 && chunks < (1 << SIZE_BITS));
        let mut tmp = start >> BASE_EXPONENT;
        let mut exponent = 0;
        while (tmp != 0) && ((tmp & 1) == 0) {
            tmp >>= 1;
            exponent += 1;
        }
        let mantissa = tmp;
        debug_assert!((tmp << (BASE_EXPONENT + exponent)) == start.as_usize());
        SpaceDescriptor(
            (mantissa << MANTISSA_SHIFT)
                | (exponent << EXPONENT_SHIFT)
                | (chunks << SIZE_SHIFT)
                | (if top {
                    TYPE_CONTIGUOUS_HI
                } else {
                    TYPE_CONTIGUOUS
                }),
        )
    }

    pub fn create_descriptor() -> SpaceDescriptor {
        let next =
            DISCONTIGUOUS_SPACE_INDEX.fetch_add(DISCONTIG_INDEX_INCREMENT, Ordering::Relaxed);
        let ret = SpaceDescriptor(next);
        debug_assert!(!ret.is_contiguous());
        ret
    }

    pub fn is_empty(self) -> bool {
        self.0 == SpaceDescriptor::UNINITIALIZED.0
    }

    pub fn is_contiguous(self) -> bool {
        (self.0 & TYPE_CONTIGUOUS) == TYPE_CONTIGUOUS
    }

    pub fn is_contiguous_hi(self) -> bool {
        (self.0 & TYPE_MASK) == TYPE_CONTIGUOUS_HI
    }

    pub fn get_start(self) -> Address {
        assert!(
            self.is_contiguous(),
            "SpaceDescriptor::get_start requires a contiguous space descriptor"
        );
        let layout = vm_layout();
        let raw_addr = if !layout.force_use_contiguous_spaces {
            // For 32-bit or 64-bit discontiguous/compressed space, decode mantissa/exponent start address
            let descriptor = self.0;
            let mantissa = descriptor >> MANTISSA_SHIFT;
            let exponent = (descriptor & EXPONENT_MASK) >> EXPONENT_SHIFT;
            let shift = BASE_EXPONENT + exponent;
            assert!(
                shift < usize::BITS as usize,
                "SpaceDescriptor shift ({shift}) overflows usize::BITS ({})",
                usize::BITS
            );
            mantissa << shift
        } else {
            let log_space_extent = layout.log_space_extent;
            assert!(
                log_space_extent < usize::BITS as usize,
                "log_space_extent ({log_space_extent}) overflows usize::BITS ({})",
                usize::BITS
            );
            self.get_index() << log_space_extent
        };
        // SAFETY: The `assert!`s above verified that `self` has the contiguous tag bit set and
        // the bit-shift is `< usize::BITS`. `Address::from_usize` wraps the reconstructed raw
        // address integer without dereferencing memory.
        unsafe { Address::from_usize(raw_addr) }
    }

    #[cfg(target_pointer_width = "64")]
    pub fn get_extent(self) -> usize {
        if !vm_layout().force_use_contiguous_spaces {
            // For 64-bit discontiguous space, use 32-bit extent
            self.get_extent_32()
        } else {
            vm_layout().space_size_64()
        }
    }

    #[cfg(target_pointer_width = "32")]
    pub fn get_extent(self) -> usize {
        self.get_extent_32()
    }

    fn get_extent_32(self) -> usize {
        debug_assert!(self.is_contiguous());
        let chunks = (self.0 & SIZE_MASK) >> SIZE_SHIFT;
        chunks << vm_layout::LOG_BYTES_IN_CHUNK
    }

    pub fn get_index(self) -> usize {
        (self.0 & INDEX_MASK) >> INDEX_SHIFT
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::heap::layout::vm_layout::*;

    #[test]
    fn create_discontiguous_descriptor() {
        let d1 = SpaceDescriptor::create_descriptor();
        assert!(!d1.is_empty());
        assert!(!d1.is_contiguous());
        assert!(!d1.is_contiguous_hi());

        let d2 = SpaceDescriptor::create_descriptor();
        assert!(!d2.is_empty());
        assert!(!d2.is_contiguous());
        assert!(!d2.is_contiguous_hi());
    }

    const TEST_SPACE_SIZE: usize = BYTES_IN_CHUNK * 10;

    #[test]
    fn create_contiguous_descriptor_at_heap_start() {
        let d = SpaceDescriptor::create_descriptor_from_heap_range(
            vm_layout().heap_start,
            vm_layout().heap_start + TEST_SPACE_SIZE,
        );
        assert!(!d.is_empty());
        assert!(d.is_contiguous());
        assert!(!d.is_contiguous_hi());
        assert_eq!(d.get_start(), vm_layout().heap_start);
        if cfg!(target_pointer_width = "64") {
            assert_eq!(d.get_extent(), vm_layout().space_size_64());
        } else {
            assert_eq!(d.get_extent(), TEST_SPACE_SIZE);
        }
    }

    #[test]
    fn create_contiguous_descriptor_in_heap() {
        let d = SpaceDescriptor::create_descriptor_from_heap_range(
            vm_layout().heap_start + TEST_SPACE_SIZE,
            vm_layout().heap_start + TEST_SPACE_SIZE * 2,
        );
        assert!(!d.is_empty());
        assert!(d.is_contiguous());
        assert!(!d.is_contiguous_hi());
        if cfg!(target_pointer_width = "64") {
            assert_eq!(d.get_start(), vm_layout().heap_start);
            assert_eq!(d.get_extent(), vm_layout().space_size_64());
        } else {
            assert_eq!(d.get_start(), vm_layout().heap_start + TEST_SPACE_SIZE);
            assert_eq!(d.get_extent(), TEST_SPACE_SIZE);
        }
    }

    #[test]
    fn create_contiguous_descriptor_at_heap_end() {
        let d = SpaceDescriptor::create_descriptor_from_heap_range(
            vm_layout().heap_end - TEST_SPACE_SIZE,
            vm_layout().heap_end,
        );
        assert!(!d.is_empty());
        assert!(d.is_contiguous());
        assert!(d.is_contiguous_hi());
        if cfg!(target_pointer_width = "64") {
            assert_eq!(
                d.get_start(),
                vm_layout().heap_end - vm_layout().space_size_64()
            );
            assert_eq!(d.get_extent(), vm_layout().space_size_64());
        } else {
            assert_eq!(d.get_start(), vm_layout().heap_end - TEST_SPACE_SIZE);
            assert_eq!(d.get_extent(), TEST_SPACE_SIZE);
        }
    }
}
