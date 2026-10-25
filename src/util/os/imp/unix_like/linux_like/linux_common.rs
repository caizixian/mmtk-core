#![deny(unsafe_op_in_unsafe_fn)]

use crate::util::address::Address;
use crate::util::os::imp::unix_like::unix_common;
use crate::util::os::*;
use libc::{cpu_set_t, sched_setaffinity, CPU_SET, CPU_ZERO};
use std::io::Result;

pub fn set_vma_name(start: Address, size: usize, annotation: &MmapAnnotation) {
    // `PR_SET_VMA` is new in Linux 5.17.  We compile against a version of the `libc` crate that
    // has the `PR_SET_VMA_ANON_NAME` constant.  When runnning on an older kernel, it will not
    // recognize this attribute and will return `EINVAL`.  However, `prctl` may return `EINVAL`
    // for other reasons, too.  That includes `start` being an invalid address, and the
    // formatted `anno_cstr` being longer than 80 bytes including the trailing `'\0'`.  But
    // since this prctl is used for debugging, we log the error instead of panicking.
    let anno_str = annotation.to_string();
    let anno_cstr = std::ffi::CString::new(anno_str).unwrap();
    let result = unix_common::wrap_libc_call(
        // SAFETY: `PR_SET_VMA` + `PR_SET_VMA_ANON_NAME` expects `(unsigned long start,
        // unsigned long size, const char *name)`. `anno_cstr` is a valid NUL-terminated `CString`
        // borrowed for the duration of the syscall, and `PR_SET_VMA_ANON_NAME` only updates kernel
        // debug VMA metadata without modifying process memory mappings or contents.
        &|| unsafe {
            libc::prctl(
                libc::PR_SET_VMA,
                libc::PR_SET_VMA_ANON_NAME,
                start.to_ptr::<libc::c_void>(),
                size,
                anno_cstr.as_ptr(),
            )
        },
        0,
    );
    if let Err(e) = result {
        debug!("Error while calling prctl: {e}");
    }
}

/// Set huge page option for the given memory.
pub fn set_hugepage(start: Address, size: usize, options: HugePageSupport) -> Result<()> {
    match options {
        HugePageSupport::No => Ok(()),
        HugePageSupport::TransparentHugePages => unix_common::wrap_libc_call(
            // SAFETY: `MADV_HUGEPAGE` is a non-destructive kernel backing-page hint (`VM_HUGEPAGE`)
            // that preserves all memory contents and mappings (returning an `errno` if `[start,
            // start + size)` is unmapped or unaligned).
            &|| unsafe { libc::madvise(start.to_mut_ptr(), size, libc::MADV_HUGEPAGE) },
            0,
        ),
    }
}

impl MmapStrategy {
    /// get the flags for POSIX mmap.
    pub fn get_posix_mmap_flags(&self, fixed: bool) -> i32 {
        let mut flags = libc::MAP_PRIVATE | libc::MAP_ANONYMOUS;
        if fixed {
            if self.replace {
                flags |= libc::MAP_FIXED;
            } else {
                flags |= libc::MAP_FIXED_NOREPLACE
            }
        }
        if !self.reserve {
            flags |= libc::MAP_NORESERVE;
        }
        flags
    }
}

/// Get the memory maps for the process. The returned string is a multi-line string.
/// This is only meant to be used for debugging. For example, log process memory maps after detecting a clash.
pub fn get_process_memory_maps() -> Result<String> {
    std::fs::read_to_string("/proc/self/maps")
}

pub fn get_total_num_cpus() -> CoreNum {
    std::thread::available_parallelism()
        .map(|n| n.get().min(CoreNum::MAX as usize) as CoreNum)
        .unwrap_or(1)
}

pub fn bind_current_thread_to_core(core_id: CoreId) {
    bind_current_thread_to_cpuset(&[core_id]);
}

pub fn bind_current_thread_to_cpuset(cpuset: &[CoreId]) {
    for &cpu in cpuset {
        let cpu_idx = cpu as usize;
        assert!(
            cpu_idx < libc::CPU_SETSIZE as usize,
            "CPU core ID {cpu_idx} exceeds CPU_SETSIZE ({})",
            libc::CPU_SETSIZE
        );
    }
    // SAFETY: `libc::cpu_set_t` is a C plain-old-data bitmask struct for which all-zero bytes are
    // valid, `CPU_ZERO` initializes `cs` as an empty CPU set, every `cpu as usize < CPU_SETSIZE`
    // was asserted above so `CPU_SET` stays in bounds, and `sched_setaffinity(0, ...)` reads `&cs`
    // of exact byte size `size_of::<cpu_set_t>()` to set the calling thread's CPU affinity.
    unsafe {
        let mut cs: cpu_set_t = std::mem::MaybeUninit::zeroed().assume_init();
        CPU_ZERO(&mut cs);
        for &cpu in cpuset {
            CPU_SET(cpu as usize, &mut cs);
        }
        sched_setaffinity(0, std::mem::size_of::<cpu_set_t>(), &cs);
    }
}

pub fn dzmmap(
    start: Address,
    size: usize,
    strategy: MmapStrategy,
    annotation: &MmapAnnotation<'_>,
) -> MmapResult<Address> {
    let addr = unix_common::mmap(start, size, strategy, annotation)?;

    if !cfg!(feature = "no_mmap_annotation") {
        set_vma_name(addr, size, annotation);
    }

    set_hugepage(addr, size, strategy.huge_page).expect("Failed to set huge page option");

    // We do not need to explicitly zero for Linux (memory is guaranteed to be zeroed)

    Ok(addr)
}

pub fn dzmmap_anywhere(
    size: usize,
    align: usize,
    strategy: MmapStrategy,
    annotation: &MmapAnnotation<'_>,
) -> MmapResult<Address> {
    let addr = unix_common::mmap_anywhere(size, align, strategy, annotation)?;
    if !cfg!(feature = "no_mmap_annotation") {
        set_vma_name(addr, size, annotation);
    }
    Ok(addr)
}

pub fn dzmmap_preferred(
    start: Address,
    size: usize,
    align: usize,
    strategy: MmapStrategy,
    annotation: &MmapAnnotation<'_>,
) -> MmapResult<Address> {
    let addr = unix_common::mmap_preferred(start, size, align, strategy, annotation)?;
    if !cfg!(feature = "no_mmap_annotation") {
        set_vma_name(addr, size, annotation);
    }
    set_hugepage(addr, size, strategy.huge_page).expect("Failed to set huge page option");
    Ok(addr)
}

pub fn panic_if_unmapped(start: Address, size: usize) {
    let strategy = MmapStrategy {
        huge_page: HugePageSupport::No,
        prot: MmapProtection::ReadWrite,
        replace: false,
        reserve: true,
    };
    let annotation = MmapAnnotation::Misc {
        name: "panic_if_unmapped",
    };
    match unix_common::mmap(start, size, strategy, &annotation) {
        Ok(_) => panic!("{} of size {} is not mapped", start, size),
        Err(e) => {
            assert!(
                e.error.kind() == std::io::ErrorKind::AlreadyExists,
                "Failed to check mapped: {:?}",
                e
            );
        }
    }
}
