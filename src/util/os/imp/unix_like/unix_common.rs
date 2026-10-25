use crate::util::address::Address;
use crate::util::constants::BYTES_IN_PAGE;
use crate::util::os::*;
use std::io::Result;

impl MmapProtection {
    fn get_native_flags(&self) -> i32 {
        use libc::{PROT_EXEC, PROT_NONE, PROT_READ, PROT_WRITE};
        match self {
            Self::ReadWrite => PROT_READ | PROT_WRITE,
            Self::ReadWriteExec => PROT_READ | PROT_WRITE | PROT_EXEC,
            Self::NoAccess => PROT_NONE,
        }
    }
}

pub fn mmap(
    start: Address,
    size: usize,
    strategy: MmapStrategy,
    annotation: &MmapAnnotation<'_>,
) -> MmapResult<Address> {
    let ptr = start.to_mut_ptr();
    let prot = strategy.prot.get_native_flags();
    let flags = strategy.get_posix_mmap_flags(true);
    wrap_libc_call(
        // SAFETY: Public `OSMemory::dzmmap` implementation. `MAP_PRIVATE | MAP_ANONYMOUS` with
        // `fd == -1` and `offset == 0` maps anonymous pages (zero-filled by the kernel on Linux,
        // or explicitly zeroed by `MacOS::dzmmap` when `strategy.reserve` is true). On Linux and
        // Android (`linux_common.rs`), `!strategy.replace` passes `MAP_FIXED_NOREPLACE`, which
        // atomically fails with `EEXIST` rather than overwriting any existing mapping, whereas
        // `strategy.replace` passes `MAP_FIXED`. On macOS (`macos.rs`), `get_posix_mmap_flags(true)`
        // unconditionally passes `MAP_FIXED` regardless of `strategy.replace`. Whenever `MAP_FIXED`
        // is in effect (`strategy.replace == true` or macOS), soundness relies on MMTk callers
        // restricting `[start, start + size)` to unmapped or MMTk-quarantined/test virtual address
        // ranges so live Rust stack/heap/code/library mappings are never overwritten (flagged for
        // Stage-2 `OSMemory` tightening).
        &|| unsafe { libc::mmap(start.to_mut_ptr(), size, prot, flags, -1, 0) },
        ptr,
    )
    .map_err(|e| MmapError::new(start, size, annotation, e))?;
    Ok(start)
}

pub fn mmap_anywhere(
    size: usize,
    align: usize,
    strategy: MmapStrategy,
    annotation: &MmapAnnotation<'_>,
) -> MmapResult<Address> {
    mmap_aligned(Address::ZERO, size, align, strategy, annotation)
}

pub fn mmap_preferred(
    start: Address,
    size: usize,
    align: usize,
    strategy: MmapStrategy,
    annotation: &MmapAnnotation<'_>,
) -> MmapResult<Address> {
    mmap_aligned(start, size, align, strategy, annotation)
}

fn mmap_aligned(
    preferred_start: Address,
    size: usize,
    align: usize,
    strategy: MmapStrategy,
    annotation: &MmapAnnotation<'_>,
) -> MmapResult<Address> {
    assert!(
        align.is_power_of_two() && align % BYTES_IN_PAGE == 0,
        "mmap alignment ({align}) must be a power-of-two multiple of BYTES_IN_PAGE ({BYTES_IN_PAGE})"
    );
    debug_assert!(size % BYTES_IN_PAGE == 0);

    let Some(aligned_size) = size.checked_add(align - 1).map(|v| v & !(align - 1)) else {
        return Err(MmapError::new(
            preferred_start,
            size,
            annotation,
            std::io::Error::from(std::io::ErrorKind::OutOfMemory),
        ));
    };
    let Some(alloc_size) = aligned_size.checked_add(align) else {
        return Err(MmapError::new(
            preferred_start,
            size,
            annotation,
            std::io::Error::from(std::io::ErrorKind::OutOfMemory),
        ));
    };
    let prot = strategy.prot.get_native_flags();
    let flags = strategy.get_posix_mmap_flags(false);

    // SAFETY: `get_posix_mmap_flags(false)` omits both `MAP_FIXED` and `MAP_FIXED_NOREPLACE`, so
    // `preferred_start` acts solely as a non-destructive kernel placement hint and never replaces
    // existing mappings; `MAP_PRIVATE | MAP_ANONYMOUS` with `fd == -1` and `offset == 0` either
    // creates a new anonymous mapping of `alloc_size` bytes or returns `MAP_FAILED`.
    let ptr = unsafe { libc::mmap(preferred_start.to_mut_ptr(), alloc_size, prot, flags, -1, 0) };
    if ptr == libc::MAP_FAILED {
        return Err(MmapError::new(
            preferred_start,
            alloc_size,
            annotation,
            std::io::Error::last_os_error(),
        ));
    }

    let start = Address::from_mut_ptr(ptr);
    let aligned_start = start.align_up(align);

    let leading_unaligned_size = aligned_start - start;
    let trailing_unaligned_size = alloc_size - leading_unaligned_size - size;

    if leading_unaligned_size > 0 {
        debug_assert!(leading_unaligned_size % BYTES_IN_PAGE == 0);
        munmap(start, leading_unaligned_size)
            .map_err(|e| MmapError::new(start, leading_unaligned_size, annotation, e))?;
    }

    if trailing_unaligned_size > 0 {
        debug_assert!(trailing_unaligned_size % BYTES_IN_PAGE == 0);
        let trailing_start = aligned_start + size;
        munmap(trailing_start, trailing_unaligned_size)
            .map_err(|e| MmapError::new(trailing_start, trailing_unaligned_size, annotation, e))?;
    }

    Ok(aligned_start)
}

pub fn is_mmap_oom(os_errno: i32) -> bool {
    os_errno == libc::ENOMEM
}

pub fn munmap(start: Address, size: usize) -> Result<()> {
    // SAFETY: Public `OSMemory::munmap` implementation. Callers must only unmap page-aligned
    // virtual address ranges owned by MMTk mappings (such as alignment padding in `mmap_aligned`
    // or test/space cleanup) with no live Rust references into `[start, start + size)`.
    wrap_libc_call(&|| unsafe { libc::munmap(start.to_mut_ptr(), size) }, 0)
}

pub fn mprotect(start: Address, size: usize, prot: MmapProtection) -> Result<()> {
    wrap_libc_call(
        // SAFETY: Public `OSMemory::set_memory_access` implementation. Callers must only change
        // page protection on page-aligned MMTk-managed virtual address ranges `[start, start + size)`
        // without revoking access to memory with live Rust references.
        &|| unsafe { libc::mprotect(start.to_mut_ptr(), size, prot.get_native_flags()) },
        0,
    )
}

pub type ProcessIDType = libc::pid_t;
pub type ThreadIDType = libc::pthread_t;

pub fn get_process_id() -> Result<ProcessIDType> {
    // SAFETY: `libc::getpid` is async-signal-safe and thread-safe with no preconditions.
    Ok(unsafe { libc::getpid() })
}

pub fn get_thread_id() -> Result<ThreadIDType> {
    // SAFETY: `libc::pthread_self` is thread-safe and always succeeds with no preconditions.
    Ok(unsafe { libc::pthread_self() })
}

pub fn wrap_libc_call<T: PartialEq>(f: &dyn Fn() -> T, expect: T) -> Result<()> {
    let ret = f();
    if ret == expect {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::util::heap::layout::vm_layout::BYTES_IN_CHUNK;
    use crate::util::test_util::{serial_test, with_cleanup};
    use std::io::ErrorKind;

    fn assert_mapping_state(start: Address, size: usize, expect_mapped: bool) {
        let annotation = MmapAnnotation::Misc {
            name: "mmap_anywhere_test",
        };
        match mmap(
            start,
            size,
            MmapStrategy::QUARANTINE.replace(false),
            &annotation,
        ) {
            Ok(_) => {
                let _ = munmap(start, size);
                assert!(
                    !expect_mapped,
                    "{start} of size {size} should still be mapped"
                );
            }
            Err(e) => {
                assert_eq!(e.error.kind(), ErrorKind::AlreadyExists);
                assert!(expect_mapped, "{start} of size {size} should be unmapped");
            }
        }
    }

    #[test]
    fn mmap_anywhere_unmaps_alignment_padding() {
        serial_test(|| {
            let size = BYTES_IN_CHUNK + BYTES_IN_PAGE;
            let start = mmap_anywhere(
                size,
                BYTES_IN_CHUNK,
                MmapStrategy::QUARANTINE,
                mmap_anno_test!(),
            )
            .unwrap();

            with_cleanup(
                || {
                    assert!(start.is_aligned_to(BYTES_IN_CHUNK));
                    assert_mapping_state(start + size - BYTES_IN_PAGE, BYTES_IN_PAGE, true);
                    assert_mapping_state(start + size, BYTES_IN_PAGE, false);
                },
                || {
                    let _ = munmap(start, size);
                },
            );
        });
    }
}
