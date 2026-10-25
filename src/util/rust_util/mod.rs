//! This module works around limitations of the Rust programming language, and provides missing
//! functionalities that we may expect the Rust programming language and its standard libraries
//! to provide.
#![deny(unsafe_op_in_unsafe_fn)]

pub mod atomic_box;
pub(crate) mod fin;
pub mod rev_group;
pub mod zeroed_alloc;

/// Const function for min value of two usize numbers.
pub const fn min_of_usize(a: usize, b: usize) -> usize {
    if a > b {
        b
    } else {
        a
    }
}

#[cfg(feature = "nightly")]
pub use core::intrinsics::{likely, unlikely};

// likely() and unlikely() compiler hints in stable Rust
// [1]: https://github.com/rust-lang/hashbrown/blob/a41bd76de0a53838725b997c6085e024c47a0455/src/raw/mod.rs#L48-L70
// [2]: https://users.rust-lang.org/t/compiler-hint-for-unlikely-likely-for-if-branches/62102/3
#[cfg(not(feature = "nightly"))]
#[inline]
#[cold]
fn cold() {}

#[cfg(not(feature = "nightly"))]
#[inline]
pub fn likely(b: bool) -> bool {
    if !b {
        cold();
    }
    b
}
#[cfg(not(feature = "nightly"))]
#[inline]
pub fn unlikely(b: bool) -> bool {
    if b {
        cold();
    }
    b
}

/// Create a formatted string that makes the best effort idenfying the current process and thread.
pub fn debug_process_thread_id() -> String {
    use crate::util::os::*;
    format!(
        "PID: {}, TID: {}",
        OS::get_process_id().map_or("(Failed to get PID)".to_string(), |pid| format!("{}", pid)),
        OS::get_thread_id().map_or("(Failed to get TID)".to_string(), |tid| format!(
            "{:?}",
            tid
        )),
    )
}
