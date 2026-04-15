use crate::util::constants::BYTES_IN_ADDRESS;
use crate::util::malloc::library::*;
use crate::util::Address;
use crate::vm::VMBinding;

fn safe_calloc(count: usize, size: usize) -> Address {
    // SAFETY: FFI call to calloc is safe as long as arguments are valid.
    Address::from_mut_ptr(unsafe { calloc(count, size) })
}

/// Allocate with alignment. This also guarantees the memory is zero initialized.
pub fn align_alloc(size: usize, align: usize) -> Address {
    let mut ptr = std::ptr::null_mut::<libc::c_void>();
    let ptr_ptr = std::ptr::addr_of_mut!(ptr);
    // SAFETY: ptr_ptr is a valid pointer to a local variable. The alignment requirements are assumed to be satisfied by the caller or checked elsewhere.
    let result = unsafe { posix_memalign(ptr_ptr, align, size) };
    if result != 0 {
        return Address::ZERO;
    }
    let address = Address::from_mut_ptr(ptr);
    crate::util::memory::zero(address, size);
    address
}

/// Allocate with alignment and offset.
/// Beside returning the allocation result, this will store the malloc result at (result - BYTES_IN_ADDRESS)
/// so we know the original malloc result.
pub fn align_offset_alloc<VM: VMBinding>(size: usize, align: usize, offset: usize) -> Address {
    // we allocate extra `align` bytes here, so we are able to handle offset
    let actual_size = size + align + BYTES_IN_ADDRESS;
    let address = safe_calloc(1, actual_size);
    if address.is_zero() {
        return address;
    }
    let mod_offset = offset % align;
    let mut result =
        crate::util::alloc::allocator::align_allocation_no_fill::<VM>(address, align, mod_offset);
    if result - BYTES_IN_ADDRESS < address {
        result += align;
    }
    let malloc_res_ptr: *mut usize = (result - BYTES_IN_ADDRESS).to_mut_ptr();
    // SAFETY: malloc_res_ptr points to valid memory within the allocated block, though it may not be aligned.
    unsafe { malloc_res_ptr.write_unaligned(address.as_usize()) };
    result
}

/// Get the malloc usable size for an address that is returned by [`crate::util::malloc::malloc_ms_util::align_offset_alloc`].
pub fn offset_malloc_usable_size(address: Address) -> usize {
    let malloc_res_ptr: *mut usize = (address - BYTES_IN_ADDRESS).to_mut_ptr();
    // SAFETY: malloc_res_ptr points to valid memory within the allocated block where the original pointer was stored,
    // and malloc_res is a valid pointer returned by calloc.
    unsafe {
        let malloc_res = malloc_res_ptr.read_unaligned() as *mut libc::c_void;
        malloc_usable_size(malloc_res)
    }
}

/// Free an address that is allocated with an offset (returned by [`crate::util::malloc::malloc_ms_util::align_offset_alloc`]).
pub fn offset_free(address: Address) {
    let malloc_res_ptr: *mut usize = (address - BYTES_IN_ADDRESS).to_mut_ptr();
    // SAFETY: malloc_res_ptr points to valid memory within the allocated block where the original pointer was stored,
    // and malloc_res is a valid pointer returned by calloc and is safe to free.
    unsafe {
        let malloc_res = malloc_res_ptr.read_unaligned() as *mut libc::c_void;
        free(malloc_res)
    }
}

pub use crate::util::malloc::library::free;

/// get malloc usable size of an address
/// is_offset_malloc: whether the address is allocated with some offset
pub fn get_malloc_usable_size(address: Address, is_offset_malloc: bool) -> usize {
    if is_offset_malloc {
        offset_malloc_usable_size(address)
    } else {
        // SAFETY: address is a valid pointer returned by the allocator.
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
        address = safe_calloc(1, size);
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
