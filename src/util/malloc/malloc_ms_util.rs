use crate::util::constants::BYTES_IN_ADDRESS;
use crate::util::malloc::library::*;
use crate::util::Address;
use crate::vm::VMBinding;

/// Allocate with alignment. This also guarantees the memory is zero initialized.
pub fn align_alloc(size: usize, align: usize) -> Address {
    assert!(
        align >= BYTES_IN_ADDRESS && align.is_power_of_two(),
        "posix_memalign requires align ({align}) to be a power of two >= {BYTES_IN_ADDRESS}"
    );
    let mut ptr = std::ptr::null_mut::<libc::c_void>();
    // SAFETY: `std::ptr::addr_of_mut!(ptr)` is a valid writable pointer to a stack-local
    // `*mut c_void`, and `align` was asserted above to be a power-of-two multiple of
    // `sizeof(void*)` as required by POSIX `posix_memalign`.
    let result = unsafe { posix_memalign(std::ptr::addr_of_mut!(ptr), align, size) };
    if result != 0 || ptr.is_null() {
        return Address::ZERO;
    }
    if size > 0 {
        // SAFETY: `posix_memalign` succeeded (`result == 0`) with `size > 0` and returned a
        // non-null `ptr` owning at least `size` writable bytes; `u8` has alignment 1.
        unsafe {
            std::ptr::write_bytes(ptr.cast::<u8>(), 0, size);
        }
    }
    Address::from_mut_ptr(ptr)
}

/// Allocate with alignment and offset.
/// Beside returning the allocation result, this will store the malloc result at (result - BYTES_IN_ADDRESS)
/// so we know the original malloc result.
pub fn align_offset_alloc<VM: VMBinding>(size: usize, align: usize, offset: usize) -> Address {
    assert!(
        align.is_power_of_two(),
        "Alignment ({align}) must be a power of two"
    );
    // Allocate extra `align + BYTES_IN_ADDRESS` bytes so we can satisfy `(result + offset) % align == 0`
    // while keeping at least `BYTES_IN_ADDRESS` bytes immediately before `result` to store `raw`.
    let Some(actual_size) = size
        .checked_add(align)
        .and_then(|s| s.checked_add(BYTES_IN_ADDRESS))
    else {
        return Address::ZERO;
    };
    // SAFETY: `calloc` is safe to call for any `(nmemb, size)` arguments and returns either a
    // valid zero-initialized heap allocation of `actual_size` bytes or null on failure.
    let raw = unsafe { calloc(1, actual_size) };
    let address = Address::from_mut_ptr(raw);
    if address.is_zero() {
        return address;
    }
    let mod_offset = offset % align;
    let mut result =
        crate::util::alloc::allocator::align_allocation_no_fill::<VM>(address, align, mod_offset);
    // Advance in multiples of `align` (preserving `(result + offset) % align == 0`) until at
    // least `BYTES_IN_ADDRESS` bytes precede `result`. Since `result` starts in
    // `[address, address + align)`, after the loop `result` is in
    // `[address + BYTES_IN_ADDRESS, address + BYTES_IN_ADDRESS + align)`, so both the stored
    // pointer at `[result - BYTES_IN_ADDRESS, result)` and the payload `[result, result + size)`
    // lie within `[address, address + actual_size)`.
    while result < address + BYTES_IN_ADDRESS {
        result += align;
    }
    let header_offset = (result - BYTES_IN_ADDRESS) - address;
    let result_offset = result - address;
    debug_assert!(result_offset + size <= actual_size);
    // SAFETY: `raw` points to a live `actual_size`-byte `calloc` allocation (`actual_size <= isize::MAX`
    // since `calloc(1, actual_size)` succeeded). Both `header_offset` and `result_offset` satisfy
    // `header_offset + BYTES_IN_ADDRESS == result_offset <= BYTES_IN_ADDRESS + align <= actual_size`,
    // so both `raw.cast::<u8>().add(header_offset)` and `raw.cast::<u8>().add(result_offset)` are
    // in bounds of `raw`'s allocated object, and `[header_offset, header_offset + BYTES_IN_ADDRESS)`
    // is writable via `write_unaligned` (preserving `raw`'s pointer provenance).
    unsafe {
        let malloc_res_ptr = raw
            .cast::<u8>()
            .add(header_offset)
            .cast::<*mut libc::c_void>();
        malloc_res_ptr.write_unaligned(raw);
        Address::from_mut_ptr(raw.cast::<u8>().add(result_offset))
    }
}

/// Read the underlying `malloc`/`calloc` pointer stored immediately before `address` by
/// [`align_offset_alloc`].
///
/// # Safety
///
/// `address` must have been returned by a live (not yet freed) call to [`align_offset_alloc`],
/// with no concurrent mutation of the `BYTES_IN_ADDRESS` header slot at `address - BYTES_IN_ADDRESS`.
#[inline]
unsafe fn read_offset_malloc_ptr(address: Address) -> *mut libc::c_void {
    let malloc_res_ptr: *const *mut libc::c_void = (address - BYTES_IN_ADDRESS).to_ptr();
    // SAFETY: Upheld by the caller: `[address - BYTES_IN_ADDRESS, address)` lies within a live
    // allocation initialized by `align_offset_alloc` via `write_unaligned(raw)`.
    unsafe { malloc_res_ptr.read_unaligned() }
}

/// Get the malloc usable size for an address that is returned by [`crate::util::malloc::malloc_ms_util::align_offset_alloc`].
///
/// # Safety
///
/// `address` must be a live allocation returned by [`align_offset_alloc`] from the active malloc
/// backend.
pub unsafe fn offset_malloc_usable_size(address: Address) -> usize {
    // SAFETY: Upheld by the caller: `address` is a live allocation returned by `align_offset_alloc`,
    // so `read_offset_malloc_ptr` recovers the original live base pointer allocated by `calloc`,
    // which is valid to query with `malloc_usable_size`.
    unsafe {
        let malloc_res = read_offset_malloc_ptr(address);
        malloc_usable_size(malloc_res)
    }
}

/// Free an address that is allocated with an offset (returned by [`crate::util::malloc::malloc_ms_util::align_offset_alloc`]).
pub fn offset_free(address: Address) {
    // SAFETY: `offset_free` is an internal `MallocSpace` helper (`pub(crate) mod malloc_ms_util`)
    // called only on live addresses whose `MS_OFFSET_MALLOC` metadata bit is set (or explicitly
    // offset-allocated in `mock_test_malloc_ms`), so `read_offset_malloc_ptr` recovers the live
    // `calloc` base pointer once and passes it to the matching allocator's `free`.
    // (Flagged for Stage-2 cross-slice `unsafe fn` tightening.)
    unsafe {
        let malloc_res = read_offset_malloc_ptr(address);
        free(malloc_res);
    }
}

pub use crate::util::malloc::library::free;

/// get malloc usable size of an address
/// is_offset_malloc: whether the address is allocated with some offset
pub fn get_malloc_usable_size(address: Address, is_offset_malloc: bool) -> usize {
    if is_offset_malloc {
        // SAFETY: Internal `MallocSpace` callers pass `is_offset_malloc == true` iff `address` was
        // returned by `align_offset_alloc` (`MS_OFFSET_MALLOC` bit set) and is still live.
        // (Flagged for Stage-2 cross-slice `unsafe fn` tightening.)
        unsafe { offset_malloc_usable_size(address) }
    } else {
        // SAFETY: Internal `MallocSpace` callers pass `is_offset_malloc == false` only for live
        // base pointers returned directly by `calloc`/`posix_memalign` from the active malloc
        // backend. (Flagged for Stage-2 cross-slice `unsafe fn` tightening.)
        unsafe { malloc_usable_size(address.to_mut_ptr()) }
    }
}

/// allocate `size` bytes, which is aligned to `align` at `offset`
/// return the address, and whether it is an offset allocation
pub fn alloc<VM: VMBinding>(size: usize, align: usize, offset: usize) -> (Address, bool) {
    let address: Address;
    let mut is_offset_malloc = false;
    // malloc returns 16 bytes aligned address.
    // So if the alignment is smaller than 16 bytes, we do not need to align.
    if align <= 16 && offset == 0 {
        assert!(
            align.is_power_of_two(),
            "Alignment ({align}) must be a power of two"
        );
        // SAFETY: `calloc` is safe for any `(1, size)` arguments and returns either a valid
        // zero-initialized allocation or null on OOM.
        let raw = unsafe { calloc(1, size) };
        address = Address::from_mut_ptr(raw);
        debug_assert!(address.is_aligned_to(align));
    } else if align > 16 && offset == 0 {
        address = align_alloc(size, align);
        debug_assert!(
            address.is_aligned_to(align),
            "Address: {:x} is not aligned to the given alignment: {}",
            address,
            align
        );
    } else {
        address = align_offset_alloc::<VM>(size, align, offset);
        is_offset_malloc = true;
        debug_assert!(
            (address + offset).is_aligned_to(align),
            "Address: {:x} is not aligned to the given alignment: {} at offset: {}",
            address,
            align,
            offset
        );
    }
    (address, is_offset_malloc)
}
