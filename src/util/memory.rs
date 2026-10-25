use crate::util::Address;

/// Set a range of memory to 0.
pub fn zero(start: Address, len: usize) {
    set(start, 0, len);
}

/// Set a range of memory to the given value. Similar to memset.
pub fn set(start: Address, val: u8, len: usize) {
    if len == 0 {
        return;
    }
    debug_assert!(!start.is_zero());
    // SAFETY: Public API preserved for compatibility (`pub mod memory`, also called across MMTk
    // spaces and side-metadata bulk operations). `len == 0` returns early above so zero-length
    // ranges at null/dangling addresses never invoke `write_bytes`; for `len > 0`, `u8` has
    // alignment 1 and callers must ensure `[start, start + len)` is mapped, writable, and free of
    // conflicting Rust references or concurrent accesses.
    unsafe {
        std::ptr::write_bytes(start.to_mut_ptr::<u8>(), val, len);
    }
}
