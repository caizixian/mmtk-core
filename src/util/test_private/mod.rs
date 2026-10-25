//! This module exposes private items in mmtk-core for testing and benchmarking. They must not be
//! used in production.
//!
//! # Notes on inlining
//!
//! In mmtk-core, we refrain from inserting inlining hints manually.  But we use `#[inline(always)]`
//! in this module explicitly because the functions here are simple wrappers of private functions,
//! and the compiler usually fails to make the right decision given that those functions are not
//! used often, and we don't compile the benchmarks using feedback-directed optimizations.

pub use crate::util::metadata::side_metadata::helpers::scan_non_zero_bits_in_metadata_bytes;
use crate::util::metadata::side_metadata::SideMetadataSpec;
use crate::vm::VMBinding;

use super::Address;

/// Expose `zero_meta_bits` when running `cargo bench`.
///
/// # Safety
///
/// * `meta_start_bit < 8`, `meta_end_bit < 8`, and `(meta_start_addr, meta_start_bit)` must not
///   exceed `(meta_end_addr, meta_end_bit)`.
/// * When the bit range is non-empty, every touched byte (`[meta_start_addr, meta_end_addr)` when
///   `meta_end_bit == 0`, or `[meta_start_addr, meta_end_addr + 1)` when `meta_end_bit != 0`) must
///   be non-null, mapped, and writable.
/// * Whole bytes in the range are overwritten non-atomically and must have no concurrent reads,
///   writes, or atomic accesses and no live Rust references aliasing them; partial boundary bytes
///   must be initialized and accessed concurrently only via `AtomicU8` without non-atomic aliases.
#[inline(always)]
pub unsafe fn zero_meta_bits(
    meta_start_addr: Address,
    meta_start_bit: u8,
    meta_end_addr: Address,
    meta_end_bit: u8,
) {
    // SAFETY: Guaranteed by caller.
    unsafe {
        SideMetadataSpec::zero_meta_bits(
            meta_start_addr,
            meta_start_bit,
            meta_end_addr,
            meta_end_bit,
        )
    }
}

/// Expose `set_meta_bits` when running `cargo bench`.
///
/// # Safety
///
/// * `meta_start_bit < 8`, `meta_end_bit < 8`, and `(meta_start_addr, meta_start_bit)` must not
///   exceed `(meta_end_addr, meta_end_bit)`.
/// * When the bit range is non-empty, every touched byte (`[meta_start_addr, meta_end_addr)` when
///   `meta_end_bit == 0`, or `[meta_start_addr, meta_end_addr + 1)` when `meta_end_bit != 0`) must
///   be non-null, mapped, and writable.
/// * Whole bytes in the range are overwritten non-atomically and must have no concurrent reads,
///   writes, or atomic accesses and no live Rust references aliasing them; partial boundary bytes
///   must be initialized and accessed concurrently only via `AtomicU8` without non-atomic aliases.
#[inline(always)]
pub unsafe fn set_meta_bits(
    meta_start_addr: Address,
    meta_start_bit: u8,
    meta_end_addr: Address,
    meta_end_bit: u8,
) {
    // SAFETY: Guaranteed by caller.
    unsafe {
        SideMetadataSpec::set_meta_bits(
            meta_start_addr,
            meta_start_bit,
            meta_end_addr,
            meta_end_bit,
        )
    }
}

/// Expose `address_to_meta_address` when running `cargo bench`.
#[inline(always)]
pub fn address_to_meta_address(metadata_spec: &SideMetadataSpec, data_addr: Address) -> Address {
    crate::util::metadata::side_metadata::address_to_meta_address(metadata_spec, data_addr)
}

/// Expose side metadata initialization when running `cargo bench`.
pub fn initialize_side_metadata<VM: VMBinding>() {
    use crate::util::options::Options;
    crate::util::metadata::side_metadata::initialize_side_metadata::<VM>(&Options::default());
}

#[cfg(feature = "mock_test")]
pub use crate::mmtk::MMAPPER;
