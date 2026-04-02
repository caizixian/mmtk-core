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

    let addr = Address::ZERO.add(8);
    assert!(addr.is_aligned_to(8));
}
