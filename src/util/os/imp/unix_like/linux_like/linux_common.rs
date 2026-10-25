use crate::util::address::Address;
use crate::util::os::imp::unix_like::unix_common;
use crate::util::os::*;
use libc::{cpu_set_t, sched_getaffinity, sched_setaffinity, CPU_COUNT, CPU_SET, CPU_ZERO};
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
    // print map
    use std::fs::File;
    use std::io::Read;
    let mut data = String::new();
    let mut f = File::open("/proc/self/maps")?;
    f.read_to_string(&mut data)?;
    Ok(data)
}

fn zeroed_cpu_set() -> cpu_set_t {
    // SAFETY: `libc::cpu_set_t` is a C plain-old-data bitmask struct of unsigned integers for
    // which an all-zero bit pattern is a valid initialized value, and `CPU_ZERO` initializes the
    // stack-allocated `cpu_set_t` in place to represent an empty CPU set.
    unsafe {
        let mut cs: cpu_set_t = std::mem::MaybeUninit::zeroed().assume_init();
        CPU_ZERO(&mut cs);
        cs
    }
}

pub fn get_total_num_cpus() -> CoreNum {
    let mut cs = zeroed_cpu_set();
    // SAFETY: `pid == 0` queries the calling thread, and `&mut cs` points to a writable,
    // initialized `cpu_set_t` of exact byte size `size_of::<cpu_set_t>()`.
    let res = unsafe { sched_getaffinity(0, std::mem::size_of::<cpu_set_t>(), &mut cs) };
    assert_eq!(
        res,
        0,
        "sched_getaffinity failed: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: `cs` is a valid initialized `cpu_set_t` populated by `sched_getaffinity`.
    unsafe { CPU_COUNT(&cs) as u16 }
}

pub fn bind_current_thread_to_core(core_id: CoreId) {
    bind_current_thread_to_cpuset(&[core_id]);
}

pub fn bind_current_thread_to_cpuset(cpuset: &[CoreId]) {
    let mut cs = zeroed_cpu_set();
    for &cpu in cpuset {
        let cpu_idx = cpu as usize;
        assert!(
            cpu_idx < libc::CPU_SETSIZE as usize,
            "CPU core ID {cpu_idx} exceeds CPU_SETSIZE ({})",
            libc::CPU_SETSIZE
        );
        // SAFETY: `cs` is a valid initialized `cpu_set_t` and `cpu_idx < CPU_SETSIZE` was asserted
        // above, satisfying `CPU_SET`'s in-bounds bit index contract.
        unsafe {
            CPU_SET(cpu_idx, &mut cs);
        }
    }
    // SAFETY: `pid == 0` sets affinity for the calling thread, and `&cs` points to a valid
    // initialized `cpu_set_t` of exact byte size `size_of::<cpu_set_t>()`.
    unsafe {
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
