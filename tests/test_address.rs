use mmtk::util::Address;

#[test]
fn test_align_up() {
    let addr = Address::ZERO;
    let aligned = addr.align_up(8);

    assert_eq!(addr, aligned);
}

#[test]
fn test_is_aligned() {
    let addr = Address::ZERO;
    assert!(addr.is_aligned_to(8));

    // SAFETY: `Address::from_usize` wraps an integer value without dereferencing memory; used here
    // purely to test alignment arithmetic on address `8`.
    let addr = unsafe { Address::from_usize(8) };
    assert!(addr.is_aligned_to(8));
}
