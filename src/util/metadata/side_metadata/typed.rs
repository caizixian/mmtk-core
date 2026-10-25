//! Strongly-typed internal wrapper (`TypedSideSpec`) around [`SideMetadataSpec`].
//!
//! Encodes region granularity (`R: Region`), bit width (`W: BitWidth`), and value bijection
//! (`V: MetadataCodec<W::Raw>`) at compile time (`const fn new`), eliminating repetitive integer
//! casts and width mismatches across `policy` and `chunk_map` consumers in Wave 2B.

#![deny(unsafe_op_in_unsafe_fn)]
#![allow(dead_code)] // Introduced in Wave 2A for Wave 2B cross-folder consumers (`BP 2.2`).

use std::marker::PhantomData;
use std::sync::atomic::Ordering;

use crate::util::linear_scan::Region;
use crate::util::metadata::metadata_val_traits::{Bits, MetadataValue};

use super::SideMetadataSpec;

mod private {
    pub trait Sealed {}
}

/// Compile-time side-metadata entry bit-width descriptor (`1`, `2`, `4`, `8`, `16`, `32`, `64`,
/// or `usize::BITS` bits) paired with its underlying [`MetadataValue`] carrier type.
pub(crate) trait BitWidth: private::Sealed + Copy + 'static {
    /// Base-2 logarithm of the bit width (`0..=6`).
    const LOG_BITS: usize;
    /// Underlying [`MetadataValue`] integer carrier (`u8` for `1..=8` bits, or exact-width integer).
    type Raw: MetadataValue;
}

macro_rules! define_bit_width {
    ($name:ident, $log_bits:expr, $raw:ty) => {
        #[derive(Copy, Clone, Debug, Eq, PartialEq)]
        pub(crate) struct $name;

        impl private::Sealed for $name {}

        impl BitWidth for $name {
            const LOG_BITS: usize = $log_bits;
            type Raw = $raw;
        }
    };
}

define_bit_width!(U1Bit, 0, u8);
define_bit_width!(U2Bits, 1, u8);
define_bit_width!(U4Bits, 2, u8);
define_bit_width!(U8Bits, 3, u8);
define_bit_width!(U16Bits, 4, u16);
define_bit_width!(U32Bits, 5, u32);
define_bit_width!(U64Bits, 6, u64);
define_bit_width!(UsizeBits, <usize as Bits>::LOG2 as usize, usize);

/// Total encoding/decoding between a domain metadata value (e.g. `ChunkState`, `BlockState`,
/// `bool`, `Address`, or raw integers) and its underlying [`MetadataValue`] integer representation.
pub(crate) trait MetadataCodec<Raw: MetadataValue>: Copy {
    fn encode(self) -> Raw;
    fn decode(raw: Raw) -> Self;
}

impl<Raw: MetadataValue> MetadataCodec<Raw> for Raw {
    #[inline(always)]
    fn encode(self) -> Raw {
        self
    }

    #[inline(always)]
    fn decode(raw: Raw) -> Self {
        raw
    }
}

impl MetadataCodec<u8> for bool {
    #[inline(always)]
    fn encode(self) -> u8 {
        u8::from(self)
    }

    #[inline(always)]
    fn decode(raw: u8) -> Self {
        raw != 0
    }
}

/// Statically region- and bit-width-checked wrapper around a [`SideMetadataSpec`].
#[repr(transparent)]
pub(crate) struct TypedSideSpec<
    R: Region,
    W: BitWidth,
    V: MetadataCodec<<W as BitWidth>::Raw> = <W as BitWidth>::Raw,
> {
    raw: SideMetadataSpec,
    _marker: PhantomData<fn(R, W) -> V>,
}

impl<R: Region, W: BitWidth, V: MetadataCodec<W::Raw>> Copy for TypedSideSpec<R, W, V> {}

impl<R: Region, W: BitWidth, V: MetadataCodec<W::Raw>> Clone for TypedSideSpec<R, W, V> {
    #[inline(always)]
    fn clone(&self) -> Self {
        *self
    }
}

impl<R: Region, W: BitWidth, V: MetadataCodec<W::Raw>> TypedSideSpec<R, W, V> {
    /// Construct a statically region-, bit-width-, and alignment-checked wrapper around `raw`.
    ///
    /// When evaluated in a `const` item, any mismatch in `R::LOG_BYTES`, `W::LOG_BITS`, or
    /// `raw.offset` alignment fails at compile time.
    #[inline(always)]
    pub(crate) const fn new(raw: SideMetadataSpec) -> Self {
        assert!(
            raw.log_bytes_in_region == R::LOG_BYTES,
            "TypedSideSpec region size does not match SideMetadataSpec::log_bytes_in_region"
        );
        assert!(
            raw.log_num_of_bits == W::LOG_BITS,
            "TypedSideSpec bit width does not match SideMetadataSpec::log_num_of_bits"
        );
        raw.assert_spec::<W::Raw>();
        Self {
            raw,
            _marker: PhantomData,
        }
    }

    /// Borrow the underlying [`SideMetadataSpec`].
    #[inline(always)]
    pub(crate) const fn as_spec(&self) -> &SideMetadataSpec {
        &self.raw
    }

    /// Return whether side metadata for `region` is mapped in `MMAPPER`.
    #[inline(always)]
    pub(crate) fn is_mapped(&self, region: R) -> bool {
        self.raw.is_mapped(region.start())
    }

    /// Atomically load and decode the metadata entry for `region`.
    #[inline(always)]
    pub(crate) fn load_atomic(&self, region: R, order: Ordering) -> V {
        V::decode(self.raw.load_atomic::<W::Raw>(region.start(), order))
    }

    /// Atomically encode and store `val` into the metadata entry for `region`.
    #[inline(always)]
    pub(crate) fn store_atomic(&self, region: R, val: V, order: Ordering) {
        self.raw
            .store_atomic::<W::Raw>(region.start(), val.encode(), order);
    }

    /// Atomically compare-and-exchange the metadata entry for `region`.
    #[inline(always)]
    pub(crate) fn compare_exchange_atomic(
        &self,
        region: R,
        current: V,
        new: V,
        success: Ordering,
        failure: Ordering,
    ) -> Result<V, V> {
        self.raw
            .compare_exchange_atomic::<W::Raw>(
                region.start(),
                current.encode(),
                new.encode(),
                success,
                failure,
            )
            .map(V::decode)
            .map_err(V::decode)
    }

    /// Non-atomically load and decode the metadata entry for `region`.
    ///
    /// # Safety
    /// Same preconditions as [`SideMetadataSpec::load`] at `region.start()`.
    #[inline(always)]
    pub(crate) unsafe fn load(&self, region: R) -> V {
        // SAFETY: Caller upholds `SideMetadataSpec::load` mapped, initialized, and non-racing
        // preconditions for `region.start()`.
        V::decode(unsafe { self.raw.load::<W::Raw>(region.start()) })
    }

    /// Non-atomically encode and store `val` into the metadata entry for `region`.
    ///
    /// # Safety
    /// Same preconditions as [`SideMetadataSpec::store`] at `region.start()`.
    #[inline(always)]
    pub(crate) unsafe fn store(&self, region: R, val: V) {
        // SAFETY: Caller upholds `SideMetadataSpec::store` mapped, writable, and exclusive-access
        // preconditions for `region.start()`.
        unsafe {
            self.raw.store::<W::Raw>(region.start(), val.encode());
        }
    }

    /// Bulk-zero metadata entries starting at `start_region` covering `bytes` of heap space.
    #[inline(always)]
    pub(crate) fn bzero(&self, start_region: R, bytes: usize) {
        self.raw.bzero_metadata(start_region.start(), bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mmap_anno_test;
    use crate::util::heap::layout::vm_layout;
    use crate::util::metadata::side_metadata::{
        address_to_meta_address, core_test_initialize_side_metadata, sanity, SideMetadataContext,
        SideMetadataSanity,
    };
    use crate::util::os::{HugePageSupport, MmapProtection};
    use crate::util::test_util::{serial_test, with_cleanup};
    use crate::util::Address;

    #[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
    struct TestPage(Address);

    impl Region for TestPage {
        const LOG_BYTES: usize = 12;

        fn from_aligned_address(address: Address) -> Self {
            Self::assert_aligned_non_zero(address);
            Self(address)
        }

        fn start(&self) -> Address {
            self.0
        }
    }

    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    #[repr(u8)]
    enum PageMark {
        Unmarked = 0,
        Marked = 1,
    }

    impl MetadataCodec<u8> for PageMark {
        fn encode(self) -> u8 {
            self as u8
        }

        fn decode(raw: u8) -> Self {
            match raw {
                0 => Self::Unmarked,
                1 => Self::Marked,
                other => panic!("Invalid PageMark discriminant {other}"),
            }
        }
    }

    const TEST_TYPED_SPEC: TypedSideSpec<TestPage, U1Bit, PageMark> =
        TypedSideSpec::new(SideMetadataSpec {
            name: "test_typed_page_mark",
            is_global: true,
            offset: 0,
            log_num_of_bits: 0,
            log_bytes_in_region: TestPage::LOG_BYTES,
        });

    #[test]
    fn test_typed_side_spec_round_trip() {
        serial_test(|| {
            core_test_initialize_side_metadata();
            let raw_spec = *TEST_TYPED_SPEC.as_spec();
            let ctx = SideMetadataContext {
                global: vec![raw_spec],
                local: vec![],
            };
            let mut sanity = SideMetadataSanity::new();
            sanity.verify_metadata_context("TypedSpecTest", &ctx);

            let data_addr = vm_layout::vm_layout().heap_start;
            crate::MMAPPER
                .ensure_mapped(
                    data_addr,
                    1,
                    HugePageSupport::No,
                    MmapProtection::ReadWrite,
                    mmap_anno_test!(),
                )
                .unwrap();

            let meta_addr = address_to_meta_address(&raw_spec, data_addr);

            with_cleanup(
                || {
                    ctx.try_map_metadata_space(data_addr, TestPage::BYTES, "TypedSpecTest")
                        .unwrap();
                    let page = TestPage::from_aligned_address(data_addr);
                    assert!(TEST_TYPED_SPEC.is_mapped(page));
                    assert_eq!(
                        TEST_TYPED_SPEC.load_atomic(page, Ordering::SeqCst),
                        PageMark::Unmarked
                    );

                    TEST_TYPED_SPEC.store_atomic(page, PageMark::Marked, Ordering::Release);
                    assert_eq!(
                        TEST_TYPED_SPEC.load_atomic(page, Ordering::Acquire),
                        PageMark::Marked
                    );

                    assert_eq!(
                        TEST_TYPED_SPEC.compare_exchange_atomic(
                            page,
                            PageMark::Marked,
                            PageMark::Unmarked,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        ),
                        Ok(PageMark::Marked)
                    );
                    // SAFETY: Single-threaded test with mapped side metadata and no racing writes.
                    assert_eq!(unsafe { TEST_TYPED_SPEC.load(page) }, PageMark::Unmarked);
                },
                || {
                    // Do not unmap via `ensure_unmap_metadata_space` because `try_map_metadata_space`
                    // registers the page in the process-wide `MMAPPER`; clear the word and reset
                    // `SideMetadataSanity` just like `global::tests::test_side_metadata`.
                    // SAFETY: `try_map_metadata_space` mapped at least one 4096-byte page starting at
                    // page-aligned `meta_addr` (`>= align_of::<u64>()`), and `serial_test` holds
                    // exclusive single-threaded access.
                    unsafe {
                        meta_addr.store::<u64>(0);
                    }
                    sanity::reset();
                },
            );
        });
    }

    #[test]
    #[should_panic(expected = "region size")]
    fn test_typed_side_spec_region_mismatch_panics() {
        let bad = SideMetadataSpec {
            name: "bad_region",
            is_global: true,
            offset: 0,
            log_num_of_bits: 0,
            log_bytes_in_region: 3,
        };
        let _ = TypedSideSpec::<TestPage, U1Bit, bool>::new(bad);
    }
}
