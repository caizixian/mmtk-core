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
    assert!(
        !start.is_zero(),
        "memory::set called with null start address and non-zero len ({len})"
    );
    // SAFETY: Public API preserved for compatibility (`pub mod memory`, also called across MMTk
    // spaces and side-metadata bulk operations). `len == 0` returns early above and `!start.is_zero()`
    // is asserted unconditionally; for `len > 0`, `u8` has alignment 1 and callers must ensure
    // `[start, start + len)` is mapped, writable, and free of conflicting Rust references or
    // concurrent accesses.
    unsafe {
        std::ptr::write_bytes(start.to_mut_ptr::<u8>(), val, len);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_length_at_zero_address_is_noop() {
        zero(Address::ZERO, 0);
        set(Address::ZERO, 0xaa, 0);
    }

    #[test]
    #[should_panic(expected = "memory::set called with null start address")]
    fn non_zero_length_at_zero_address_panics() {
        zero(Address::ZERO, 1);
    }
}
