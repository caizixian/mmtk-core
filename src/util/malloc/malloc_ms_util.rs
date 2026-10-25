use crate::util::constants::BYTES_IN_ADDRESS;
use crate::util::malloc::library::*;
use crate::util::Address;
use crate::vm::VMBinding;

pub use crate::util::malloc::library::free;

/// Allocate `size` bytes aligned to `align` at `offset`, returning
/// `(address, usable_size, is_offset_malloc)`, or `(Address::ZERO, 0, _)` on failure.
pub(crate) fn alloc_with_usable_size<VM: VMBinding>(
    size: usize,
    align: usize,
    offset: usize,
) -> (Address, usize, bool) {
    assert!(
        align.is_power_of_two(),
        "Alignment ({align}) must be a power of two"
    );
    // malloc returns 16-byte-aligned addresses, so smaller zero-offset alignments use calloc directly.
    if align <= 16 && offset == 0 {
        // SAFETY: `calloc` is well-defined for any `(1, size)` arguments and returns either null
        // or a live zero-initialized allocation valid to query with `malloc_usable_size`.
        let (address, usable) = unsafe {
            let raw = calloc(1, size);
            if raw.is_null() {
                return (Address::ZERO, 0, false);
            }
            (Address::from_mut_ptr(raw), malloc_usable_size(raw))
        };
        debug_assert!(address.is_aligned_to(align));
        (address, usable, false)
    } else if align > 16 && offset == 0 {
        assert!(
            align >= BYTES_IN_ADDRESS,
            "posix_memalign requires align ({align}) >= {BYTES_IN_ADDRESS}"
        );
        if size > isize::MAX as usize {
            return (Address::ZERO, 0, false);
        }
        let mut ptr = std::ptr::null_mut::<libc::c_void>();
        // SAFETY: `addr_of_mut!(ptr)` is a valid writable stack pointer and `align` is a power of
        // two `>= sizeof(void*)` as required by POSIX `posix_memalign`. On success (`result == 0`
        // and `!ptr.is_null()`), `ptr` owns at least `size <= isize::MAX` writable bytes (`u8` has
        // alignment 1) and is a live allocation valid to query with `malloc_usable_size`.
        let (address, usable) = unsafe {
            let result = posix_memalign(std::ptr::addr_of_mut!(ptr), align, size);
            if result != 0 || ptr.is_null() {
                return (Address::ZERO, 0, false);
            }
            if size > 0 {
                std::ptr::write_bytes(ptr.cast::<u8>(), 0, size);
            }
            (Address::from_mut_ptr(ptr), malloc_usable_size(ptr))
        };
        debug_assert!(
            address.is_aligned_to(align),
            "Address: {:x} is not aligned to the given alignment: {}",
            address,
            align
        );
        (address, usable, false)
    } else {
        // Allocate extra `align + BYTES_IN_ADDRESS` bytes so we can satisfy `(result + offset) % align == 0`
        // while keeping at least `BYTES_IN_ADDRESS` bytes immediately before `result` to store `raw`.
        let Some(actual_size) = size
            .checked_add(align)
            .and_then(|s| s.checked_add(BYTES_IN_ADDRESS))
        else {
            return (Address::ZERO, 0, true);
        };
        if actual_size > isize::MAX as usize {
            return (Address::ZERO, 0, true);
        }
        // SAFETY: `calloc(1, actual_size)` returns either null or a live zero-initialized
        // allocation of `actual_size <= isize::MAX` bytes. When non-null, advancing `result` in
        // multiples of `align` places `result` in `[address + BYTES_IN_ADDRESS, address +
        // BYTES_IN_ADDRESS + align)`, so `header_offset + BYTES_IN_ADDRESS == result_offset <=
        // actual_size`; writing `raw` via `write_unaligned` at `header_offset`, forming the payload
        // pointer at `result_offset`, and querying `malloc_usable_size(raw)` are all in-bounds of
        // `raw`'s live allocation while preserving pointer provenance.
        let (address, usable) = unsafe {
            let raw = calloc(1, actual_size);
            if raw.is_null() {
                return (Address::ZERO, 0, true);
            }
            let base = Address::from_mut_ptr(raw);
            let mut result = crate::util::alloc::allocator::align_allocation_no_fill::<VM>(
                base,
                align,
                offset % align,
            );
            while result < base + BYTES_IN_ADDRESS {
                result += align;
            }
            let header_offset = (result - BYTES_IN_ADDRESS) - base;
            let result_offset = result - base;
            debug_assert!(result_offset + size <= actual_size);
            raw.cast::<u8>()
                .add(header_offset)
                .cast::<*mut libc::c_void>()
                .write_unaligned(raw);
            let usable = malloc_usable_size(raw);
            (
                Address::from_mut_ptr(raw.cast::<u8>().add(result_offset)),
                usable,
            )
        };
        debug_assert!(
            (address + offset).is_aligned_to(align),
            "Address: {:x} is not aligned to the given alignment: {} at offset: {}",
            address,
            align,
            offset
        );
        (address, usable, true)
    }
}

/// Allocate `size` bytes, aligned to `align` at `offset`.
/// Returns `(address, is_offset_malloc)`, or `(Address::ZERO, _)` on allocation failure.
pub(crate) fn alloc<VM: VMBinding>(size: usize, align: usize, offset: usize) -> (Address, bool) {
    let (address, _usable_size, is_offset_malloc) =
        alloc_with_usable_size::<VM>(size, align, offset);
    (address, is_offset_malloc)
}

/// Get the malloc usable size of a live allocation returned by [`alloc_with_usable_size`] / [`alloc`].
///
/// # Safety
///
/// * `address` must be a non-zero live (not yet freed) allocation from the active malloc backend.
/// * `is_offset_malloc` must match how `address` was allocated: `true` iff `address` was allocated
///   with an alignment offset (with its base pointer intact at `address - BYTES_IN_ADDRESS`), and
///   `false` iff `address` is the direct base pointer returned by `calloc`/`posix_memalign`.
pub(crate) unsafe fn get_malloc_usable_size(address: Address, is_offset_malloc: bool) -> usize {
    // SAFETY: Caller guarantees `address` is a live allocation matching `is_offset_malloc`; when
    // `is_offset_malloc` is true, `[address - BYTES_IN_ADDRESS, address)` stores the original base
    // pointer written via `write_unaligned` during allocation.
    unsafe {
        let raw = if is_offset_malloc {
            (address - BYTES_IN_ADDRESS)
                .to_ptr::<*mut libc::c_void>()
                .read_unaligned()
        } else {
            address.to_mut_ptr()
        };
        malloc_usable_size(raw)
    }
}

/// Free an address that was allocated with an alignment offset (`is_offset_malloc == true`).
///
/// # Safety
///
/// * `address` must be a non-zero live offset allocation from [`alloc_with_usable_size`] / [`alloc`]
///   whose stored base pointer at `address - BYTES_IN_ADDRESS` is intact and unmodified.
/// * `address` must not be accessed or freed again after this call.
pub(crate) unsafe fn offset_free(address: Address) {
    // SAFETY: Caller guarantees `address` is a live offset allocation whose base pointer at
    // `address - BYTES_IN_ADDRESS` was initialized via `write_unaligned` and has not yet been freed.
    unsafe {
        let raw = (address - BYTES_IN_ADDRESS)
            .to_ptr::<*mut libc::c_void>()
            .read_unaligned();
        free(raw);
    }
}
