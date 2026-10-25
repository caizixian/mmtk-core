use super::sft::*;
use crate::util::metadata::side_metadata::SideMetadataSpec;
use crate::util::Address;

use std::sync::atomic::Ordering;
use std::sync::OnceLock;

/// Concrete enum dispatch for the global SFT map, eliminating `Box<dyn SFTMap>` double
/// indirection and global mutable aliasing.
pub(crate) enum SFTMapImpl {
    Empty,
    #[cfg(target_pointer_width = "64")]
    Space(Box<space_map::SFTSpaceMap>),
    Dense(Box<dense_chunk_map::SFTDenseChunkMap>),
    Sparse(sparse_chunk_map::SFTSparseChunkMap),
}

assert_impl_all!(SFTMapImpl: Send, Sync);

impl SFTMapImpl {
    #[inline]
    pub(crate) fn has_sft_entry(&self, addr: Address) -> bool {
        match self {
            Self::Empty => false,
            #[cfg(target_pointer_width = "64")]
            Self::Space(map) => map.has_sft_entry(addr),
            Self::Dense(map) => map.has_sft_entry(addr),
            Self::Sparse(map) => map.has_sft_entry(addr),
        }
    }

    #[inline]
    pub(crate) fn get_side_metadata(&self) -> Option<&'static SideMetadataSpec> {
        match self {
            Self::Empty => None,
            #[cfg(target_pointer_width = "64")]
            Self::Space(map) => map.get_side_metadata(),
            Self::Dense(map) => map.get_side_metadata(),
            Self::Sparse(map) => map.get_side_metadata(),
        }
    }

    #[inline(always)]
    pub(crate) fn get_checked(&self, address: Address) -> &'static dyn SFT {
        match self {
            Self::Empty => &EMPTY_SPACE_SFT,
            #[cfg(target_pointer_width = "64")]
            Self::Space(map) => map.get_checked(address),
            Self::Dense(map) => map.get_checked(address),
            Self::Sparse(map) => map.get_checked(address),
        }
    }

    pub(crate) fn notify_space_creation_raw(&self, space: SFTRawPointer) {
        if let Self::Dense(map) = self {
            map.notify_space_creation(space);
        }
    }

    pub(crate) fn update_static(
        &self,
        space: &'static (dyn SFT + Sync + 'static),
        start: Address,
        bytes: usize,
    ) {
        match self {
            Self::Empty => panic!("SFTMapImpl::update_static called before initialization"),
            #[cfg(target_pointer_width = "64")]
            Self::Space(map) => map.update_static(space, start, bytes),
            Self::Dense(map) => map.update_static(space, start, bytes),
            Self::Sparse(map) => map.update_static(space, start, bytes),
        }
    }

    pub(crate) fn eager_initialize_static(
        &self,
        space: &'static (dyn SFT + Sync + 'static),
        start: Address,
        bytes: usize,
    ) {
        match self {
            Self::Dense(map) => map.eager_initialize_static(space, start, bytes),
            _ => self.update_static(space, start, bytes),
        }
    }

    pub(crate) fn clear_checked(&self, address: Address) {
        match self {
            Self::Empty => {}
            #[cfg(target_pointer_width = "64")]
            Self::Space(map) => map.clear_checked(address),
            Self::Dense(map) => map.clear_checked(address),
            Self::Sparse(map) => map.clear_checked(address),
        }
    }
}

fn create_sft_map_impl() -> SFTMapImpl {
    cfg_if::cfg_if! {
        if #[cfg(target_pointer_width = "64")] {
            use crate::util::heap::layout::vm_layout::vm_layout;
            if cfg!(any(feature = "malloc_mark_sweep", feature = "vm_space")) {
                // Off-heap memory (malloc'd objects or VM space) can reside anywhere in the
                // 47/48-bit user address space, including when compressed pointers are enabled.
                SFTMapImpl::Dense(Box::new(dense_chunk_map::SFTDenseChunkMap::new()))
            } else if !vm_layout().force_use_contiguous_spaces {
                // Discontiguous compressed-pointer heap without off-heap spaces.
                SFTMapImpl::Sparse(sparse_chunk_map::SFTSparseChunkMap::new())
            } else {
                // Contiguous 64-bit spaces indexed by 5-bit space prefix.
                SFTMapImpl::Space(Box::new(space_map::SFTSpaceMap::new()))
            }
        } else if #[cfg(target_pointer_width = "32")] {
            if cfg!(any(feature = "malloc_mark_sweep", feature = "vm_space")) {
                SFTMapImpl::Dense(Box::new(dense_chunk_map::SFTDenseChunkMap::new()))
            } else {
                SFTMapImpl::Sparse(sparse_chunk_map::SFTSparseChunkMap::new())
            }
        } else {
            compile_error!("Cannot figure out which SFT map to use.");
        }
    }
}

static EMPTY_SFT_MAP_FALLBACK: SFTMapImpl = SFTMapImpl::Empty;
static SFT_MAP_OWNER: OnceLock<SFTMapImpl> = OnceLock::new();

/// Initialize the global SFT map once and return its `'static` reference.
pub(crate) fn ensure_sft_map_initialized() -> &'static SFTMapImpl {
    SFT_MAP_OWNER.get_or_init(|| {
        SFTRefStorage::pre_use_check();
        create_sft_map_impl()
    })
}

/// Zero-sized global handle for lock-free SFT dispatch (`crate::mmtk::SFT_MAP`).
pub struct GlobalSftMap;

impl GlobalSftMap {
    #[inline(always)]
    fn map_acquire(&self) -> &'static SFTMapImpl {
        SFT_MAP_OWNER.get().unwrap_or(&EMPTY_SFT_MAP_FALLBACK)
    }

    #[inline]
    pub fn has_sft_entry(&self, addr: Address) -> bool {
        self.map_acquire().has_sft_entry(addr)
    }

    #[inline]
    pub fn get_side_metadata(&self) -> Option<&'static SideMetadataSpec> {
        self.map_acquire().get_side_metadata()
    }

    #[inline(always)]
    pub fn get_checked(&self, address: Address) -> &'static dyn SFT {
        self.map_acquire().get_checked(address)
    }

    /// Register a `'static` space in `SFT_MAP` safely (`0` `unsafe`).
    pub fn notify_space_creation_static(&self, space: &'static (dyn SFT + Sync + 'static)) {
        ensure_sft_map_initialized().notify_space_creation_raw(space);
    }

    /// Eagerly initialize SFT entries (and dense-chunk side metadata if needed) for a `'static` space.
    pub fn eager_initialize_static(
        &self,
        space: &'static (dyn SFT + Sync + 'static),
        start: Address,
        bytes: usize,
    ) {
        ensure_sft_map_initialized().eager_initialize_static(space, start, bytes);
    }

    /// Update SFT entries for `[start, start + bytes)` with a `'static` space reference.
    pub fn update_static(
        &self,
        space: &'static (dyn SFT + Sync + 'static),
        start: Address,
        bytes: usize,
    ) {
        self.map_acquire().update_static(space, start, bytes);
    }

    /// Safely clear the SFT entry for `address` if it is mapped.
    pub fn clear_checked(&self, address: Address) {
        self.map_acquire().clear_checked(address);
    }
}

/// The raw pointer for SFT. We expect a space to provide this to SFT map.
pub(crate) type SFTRawPointer = *const (dyn SFT + Sync + 'static);

/// We store raw pointer as a double word using atomics.
/// We use portable_atomic. It provides non locking atomic operations where possible,
/// and use a locking operation as the fallback.
/// Rust only provides AtomicU128 for some platforms, and do not provide the type
/// on x86_64-linux, as some earlier x86_64 CPUs do not have 128 bits atomic instructions.
/// The crate portable_atomic works around the problem with a runtime detection to
/// see if 128 bits atomic instructions are available.
#[cfg(target_pointer_width = "64")]
type AtomicDoubleWord = portable_atomic::AtomicU128;
#[cfg(target_pointer_width = "64")]
type DoubleWord = u128;
#[cfg(target_pointer_width = "32")]
type AtomicDoubleWord = portable_atomic::AtomicU64;
#[cfg(target_pointer_width = "32")]
type DoubleWord = u64;

const _: () = assert!(
    std::mem::size_of::<AtomicDoubleWord>() == std::mem::size_of::<SFTRawPointer>(),
    "AtomicDoubleWord must match SFTRawPointer fat-pointer width"
);

/// Double-word atomic cell storing a `'static` SFT fat pointer.
///
/// # Representation Invariant
/// `self.0` always stores the bit-pattern of a valid `&'static (dyn SFT + Sync + 'static)` trait
/// object reference (either `&EMPTY_SPACE_SFT` or a `'static` space instance in `MMTK<VM>`).
#[repr(transparent)]
pub(crate) struct SFTRefStorage(AtomicDoubleWord);

impl SFTRefStorage {
    /// A check at boot time to ensure `SFTRefStorage` is correct.
    pub fn pre_use_check() {
        // If we do not have lock free operations, warn the users.
        if !AtomicDoubleWord::is_lock_free() {
            warn!(
                "SFT access word is not lock free on this platform. This will slow down SFT map."
            );
        }
        // Our storage type needs to be the same width as the dyn pointer type.
        assert_eq!(
            std::mem::size_of::<AtomicDoubleWord>(),
            std::mem::size_of::<SFTRawPointer>()
        );
    }

    #[inline(always)]
    fn encode_static(sft: &'static (dyn SFT + Sync + 'static)) -> DoubleWord {
        let ptr: SFTRawPointer = sft;
        // SAFETY: `DoubleWord` has the exact same size as `SFTRawPointer` (verified by the
        // compile-time assertion above) and has no invalid bit patterns.
        unsafe { std::mem::transmute(ptr) }
    }

    /// Create an `SFTRefStorage` initialized with `&EMPTY_SPACE_SFT`.
    pub fn empty() -> Self {
        Self::from_static(&EMPTY_SPACE_SFT)
    }

    /// Create an `SFTRefStorage` initialized with a `'static` SFT reference.
    pub fn from_static(sft: &'static (dyn SFT + Sync + 'static)) -> Self {
        Self(AtomicDoubleWord::new(Self::encode_static(sft)))
    }

    /// Load with `Acquire` ordering.
    #[inline(always)]
    pub fn load(&self) -> &'static dyn SFT {
        let val = self.0.load(Ordering::Acquire);
        // Provenance-related APIs were stabilized in Rust 1.84.
        // Rust 1.91 introduced the warn-by-default lint `integer_to_ptr_transmutes`.
        // However, pointer provenance API only works for ptr-sized intergers, and
        // here we are transmuting from a double-word sized integer to a fat pointer.
        // We still need to use transmute here.
        // SAFETY: By `SFTRefStorage`'s representation invariant (maintained solely by `from_static`
        // and `store_static` with `Release` ordering paired with `Acquire` above), `val` is the
        // exact bit-pattern (including provenance and vtable) of a valid
        // `&'static (dyn SFT + Sync + 'static)` reference.
        #[allow(unknown_lints)]
        #[allow(integer_to_ptr_transmutes)]
        let sft: &'static (dyn SFT + Sync + 'static) = unsafe { std::mem::transmute(val) };
        sft
    }

    /// Store a `'static` SFT reference with `Release` ordering.
    pub fn store_static(&self, sft: &'static (dyn SFT + Sync + 'static)) {
        self.0.store(Self::encode_static(sft), Ordering::Release)
    }
}

impl std::default::Default for SFTRefStorage {
    fn default() -> Self {
        Self::empty()
    }
}

#[cfg(target_pointer_width = "64")]
mod space_map {
    use super::*;
    use crate::util::heap::layout::heap_parameters::MAX_SPACES;
    use crate::util::heap::layout::vm_layout::vm_layout;
    use crate::util::rust_util::fin::Fin;

    /// Space map is a small 32-entry table with one entry per contiguous 64-bit MMTk space,
    /// caching `address_mask` and `log_space_extent` so lookups perform zero `vm_layout()` loads
    /// and zero bounds checks via `Fin<32>`.
    pub(crate) struct SFTSpaceMap {
        address_mask: usize,
        log_space_extent: usize,
        space_address_start: Address,
        space_address_end: Address,
        sft: [SFTRefStorage; Self::TABLE_SIZE],
    }

    impl SFTSpaceMap {
        const TABLE_SIZE: usize = 32;

        /// Create a new space map caching the active `VMLayout` mask and shift parameters.
        #[allow(clippy::assertions_on_constants)]
        pub fn new() -> Self {
            let layout = vm_layout();
            let log_space_extent = layout.log_space_extent;
            assert!(
                log_space_extent <= usize::BITS as usize - 5,
                "log_space_extent ({}) too large for 5-bit SFTSpaceMap index",
                log_space_extent
            );
            let address_mask = layout.address_mask();
            let space_address_start = Self::index_to_space_range_with(1, log_space_extent).0;
            let space_address_end =
                Self::index_to_space_range_with(MAX_SPACES - 1, log_space_extent).1;
            let map = Self {
                address_mask,
                log_space_extent,
                space_address_start,
                space_address_end,
                sft: std::array::from_fn(|_| SFTRefStorage::empty()),
            };
            let table_size = map.addr_to_index(Address::MAX).get() + 1;
            assert_eq!(table_size, Self::TABLE_SIZE);
            debug_assert!(table_size >= MAX_SPACES);
            map
        }

        #[inline(always)]
        pub fn has_sft_entry(&self, addr: Address) -> bool {
            addr >= self.space_address_start && addr < self.space_address_end
        }

        #[inline(always)]
        pub fn get_side_metadata(&self) -> Option<&'static SideMetadataSpec> {
            None
        }

        #[inline(always)]
        pub fn get_checked(&self, address: Address) -> &'static dyn SFT {
            if self.has_sft_entry(address) {
                self.sft[self.addr_to_index(address)].load()
            } else {
                &EMPTY_SPACE_SFT
            }
        }

        pub fn update_static(
            &self,
            space: &'static (dyn SFT + Sync + 'static),
            start: Address,
            bytes: usize,
        ) {
            if bytes == 0 {
                return;
            }
            let end_raw = start
                .as_usize()
                .checked_add(bytes)
                .expect("SFTSpaceMap::update start + bytes overflowed Address");
            let index = self.addr_to_index(start);
            assert!(
                index.get() != 0,
                "SFTSpaceMap::update invalid space index 0 for {start}"
            );
            let (space_start, space_end) =
                Self::index_to_space_range_with(index.get(), self.log_space_extent);
            assert!(
                start >= space_start && end_raw <= space_end.as_usize(),
                "The range of {} + {} bytes does not fall into the space range {} and {}, \
                and it is probably outside the address range we use.",
                start,
                bytes,
                space_start,
                space_end
            );
            #[cfg(debug_assertions)]
            {
                let old = self.sft[index].load();
                assert!(old.name() == EMPTY_SFT_NAME || old.name() == space.name());
            }
            self.sft[index].store_static(space);
        }

        pub fn clear_checked(&self, addr: Address) {
            if self.has_sft_entry(addr) {
                self.sft[self.addr_to_index(addr)].store_static(&EMPTY_SPACE_SFT);
            }
        }

        #[inline(always)]
        fn addr_to_index(&self, addr: Address) -> Fin<{ Self::TABLE_SIZE }> {
            Fin::from_pow2_mask(addr.and(self.address_mask) >> self.log_space_extent)
        }

        fn index_to_space_range_with(i: usize, log_space_extent: usize) -> (Address, Address) {
            use crate::util::heap::layout::heap_parameters::MAX_SPACES;
            assert!(
                i != 0 && i <= MAX_SPACES && log_space_extent <= usize::BITS as usize - 5,
                "Invalid space index {i} or log_space_extent {log_space_extent}"
            );
            let start_raw = i << log_space_extent;
            let extent = 1usize << log_space_extent;
            // SAFETY: `i <= MAX_SPACES (16)` and `log_space_extent <= usize::BITS - 5` guarantee
            // `start_raw + extent == (i + 1) << log_space_extent <= 17 << (BITS - 5) < usize::MAX`;
            // neither virtual space boundary is dereferenced directly here.
            unsafe {
                (
                    Address::from_usize(start_raw),
                    Address::from_usize(start_raw + extent),
                )
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::util::heap::layout::heap_parameters::MAX_SPACES;
        use crate::util::heap::layout::vm_layout::vm_layout;

        #[test]
        fn test_address_arithmetic() {
            let map = SFTSpaceMap::new();
            let log_space_extent = map.log_space_extent;
            assert_eq!(map.addr_to_index(Address::ZERO).get(), 0);
            assert_eq!(map.addr_to_index(vm_layout().heap_start - 1).get(), 0);

            let assert_for_index = |i: usize| {
                let (start, end) = SFTSpaceMap::index_to_space_range_with(i, log_space_extent);
                assert_eq!(map.addr_to_index(start).get(), i);
                assert_eq!(map.addr_to_index(end - 1).get(), i);
            };

            for i in 1..=MAX_SPACES {
                assert_for_index(i);
            }

            let (_, last_space_end) =
                SFTSpaceMap::index_to_space_range_with(MAX_SPACES, log_space_extent);
            assert_eq!(last_space_end, vm_layout().heap_end);

            assert_eq!(map.addr_to_index(last_space_end).get(), 17);
            assert_eq!(map.addr_to_index(Address::MAX).get(), 31);
        }
    }
}

mod dense_chunk_map {
    use super::*;
    use crate::util::conversions;
    use crate::util::metadata::side_metadata::spec_defs::SFT_DENSE_CHUNK_MAP_INDEX;
    use crate::util::metadata::side_metadata::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    struct DenseIndexRegistry {
        /// Next unused dense-table index (`1..=255`).
        next_index: usize,
        /// Map from space data-pointer address (`space as *const () as usize`) to its `u8` index.
        index_map: HashMap<usize, u8>,
    }

    /// SFTDenseChunkMap has one entry per space in a 256-slot table and uses `u8` side metadata to
    /// record the space index for each 4 MiB chunk.
    pub(crate) struct SFTDenseChunkMap {
        /// Fixed 256-entry dense table indexed directly by `u8` side-metadata values (`0` is
        /// `EMPTY_SPACE_SFT`, so `u8 as usize < 256` elides bounds checks with zero `unsafe`).
        sft: [SFTRefStorage; Self::MAX_SPACES],
        /// Published count of registered indices (`1 + registered_spaces`).
        published_count: AtomicUsize,
        /// Synchronizes cold space registration (`notify_space_creation`) without `&mut SFTDenseChunkMap`.
        registry: Mutex<DenseIndexRegistry>,
    }

    impl SFTDenseChunkMap {
        const EMPTY_SFT_INDEX: u8 = 0;
        const MAX_SPACES: usize = u8::MAX as usize + 1;

        pub fn new() -> Self {
            Self {
                sft: std::array::from_fn(|_| SFTRefStorage::empty()),
                published_count: AtomicUsize::new(1),
                registry: Mutex::new(DenseIndexRegistry {
                    next_index: 1,
                    index_map: HashMap::new(),
                }),
            }
        }

        #[inline(always)]
        fn is_index_mapped(addr: Address) -> bool {
            if crate::util::metadata::side_metadata::global_side_metadata_base_address().is_zero() {
                return false;
            }
            let meta_addr = crate::util::metadata::side_metadata::address_to_meta_address(
                &SFT_DENSE_CHUNK_MAP_INDEX,
                addr,
            );
            meta_addr >= SFT_DENSE_CHUNK_MAP_INDEX.get_starting_address()
                && meta_addr < SFT_DENSE_CHUNK_MAP_INDEX.upper_bound_address_for_contiguous()
                && SFT_DENSE_CHUNK_MAP_INDEX.is_mapped(addr)
        }

        /// Safely read the 1-byte dense SFT index if `SFT_DENSE_CHUNK_MAP_INDEX` is mapped for `addr`.
        #[inline(always)]
        fn addr_to_index_checked(addr: Address) -> Option<u8> {
            Self::is_index_mapped(addr)
                .then(|| SFT_DENSE_CHUNK_MAP_INDEX.load_atomic::<u8>(addr, Ordering::Acquire))
        }

        #[inline]
        pub fn has_sft_entry(&self, addr: Address) -> bool {
            match Self::addr_to_index_checked(addr) {
                Some(index) => (index as usize) < self.published_count.load(Ordering::Acquire),
                None => false,
            }
        }

        #[inline]
        pub fn get_side_metadata(&self) -> Option<&'static SideMetadataSpec> {
            Some(&SFT_DENSE_CHUNK_MAP_INDEX)
        }

        #[inline(always)]
        pub fn get_checked(&self, address: Address) -> &'static dyn SFT {
            if let Some(index) = Self::addr_to_index_checked(address) {
                let index = index as usize;
                if index < self.published_count.load(Ordering::Acquire) {
                    return self.sft[index].load();
                }
            }
            &EMPTY_SPACE_SFT
        }

        pub fn notify_space_creation(&self, space: SFTRawPointer) {
            let space_addr = space as *const () as usize;
            let mut reg = self.registry.lock().unwrap();
            assert!(
                !reg.index_map.contains_key(&space_addr),
                "SFTDenseChunkMap::notify_space_creation called multiple times for space at {space_addr:#x}"
            );
            let index = reg.next_index;
            assert!(
                index < Self::MAX_SPACES,
                "SFTDenseChunkMap supports at most {} spaces",
                u8::MAX
            );
            reg.next_index += 1;
            reg.index_map.insert(space_addr, index as u8);
            self.published_count
                .store(reg.next_index, Ordering::Release);
        }

        pub fn eager_initialize_static(
            &self,
            space: &'static (dyn SFT + Sync + 'static),
            start: Address,
            bytes: usize,
        ) {
            assert!(
                start.as_usize().checked_add(bytes).is_some(),
                "SFTDenseChunkMap::eager_initialize start + bytes overflowed Address"
            );
            if bytes > 0 {
                assert!(
                    crate::util::metadata::side_metadata::address_to_meta_address(
                        &SFT_DENSE_CHUNK_MAP_INDEX,
                        start + (bytes - 1),
                    ) < SFT_DENSE_CHUNK_MAP_INDEX.upper_bound_address_for_contiguous(),
                    "SFTDenseChunkMap::eager_initialize range [{start}, {}) exceeds contiguous side-metadata bound",
                    start + bytes,
                );
                let context = SideMetadataContext {
                    global: vec![SFT_DENSE_CHUNK_MAP_INDEX],
                    local: vec![],
                };
                context
                    .try_map_metadata_space(start, bytes, "SFTDenseChunkMap")
                    .unwrap_or_else(|e| {
                        panic!("failed to mmap metadata memory: {e}");
                    });
            }
            self.update_static(space, start, bytes);
        }

        pub fn update_static(
            &self,
            space: &'static (dyn SFT + Sync + 'static),
            start: Address,
            bytes: usize,
        ) {
            assert!(
                start.as_usize().checked_add(bytes).is_some(),
                "SFTDenseChunkMap::update start + bytes overflowed Address"
            );
            let space_addr = space as *const (dyn SFT + Sync + 'static) as *const () as usize;
            let index_u8 = {
                let reg = self.registry.lock().unwrap();
                *reg.index_map
                    .get(&space_addr)
                    .expect("notify_space_creation must be called before update")
            };
            self.sft[index_u8 as usize].store_static(space);

            if bytes > 0 {
                let first_chunk = start.chunk_index();
                let last_chunk = (start + (bytes - 1)).chunk_index();
                debug!(
                    "update {} (chunk #{}) to {} (chunk #{})",
                    start,
                    first_chunk,
                    start + bytes,
                    last_chunk
                );
                for chunk_idx in first_chunk..=last_chunk {
                    let chunk = conversions::chunk_index_to_address(chunk_idx);
                    assert!(
                        Self::is_index_mapped(chunk),
                        "SFTDenseChunkMap::update called on unmapped or out-of-bounds side-metadata chunk {chunk}"
                    );
                    trace!("Update {} to index {}", chunk, index_u8);
                    SFT_DENSE_CHUNK_MAP_INDEX.store_atomic::<u8>(chunk, index_u8, Ordering::SeqCst);
                }
            }
            debug!("update done");
        }

        pub fn clear_checked(&self, address: Address) {
            if Self::is_index_mapped(address) {
                SFT_DENSE_CHUNK_MAP_INDEX.store_atomic::<u8>(
                    address,
                    Self::EMPTY_SFT_INDEX,
                    Ordering::SeqCst,
                );
            }
        }
    }
}

mod sparse_chunk_map {
    use super::*;
    use crate::util::conversions::*;
    #[cfg(target_pointer_width = "64")]
    use crate::util::heap::layout::vm_layout::vm_layout;
    use crate::util::heap::layout::vm_layout::BYTES_IN_CHUNK;
    #[cfg(target_pointer_width = "32")]
    use crate::util::heap::layout::vm_layout::LOG_BYTES_IN_CHUNK;
    #[cfg(target_pointer_width = "32")]
    use crate::util::rust_util::fin::Fin;

    #[cfg(target_pointer_width = "32")]
    const ARCH_CHUNKS_32: usize = 1usize << (32 - LOG_BYTES_IN_CHUNK);

    /// Sparse chunk table with one entry per 4 MiB chunk in the addressable space.
    ///
    /// On 32-bit targets, `Box<[SFTRefStorage; 1024]>` covers the entire 32-bit address space
    /// (`u32 >> 22 < 1024`), eliminating all bounds checks and all `unsafe` indexing!
    pub(crate) struct SFTSparseChunkMap {
        #[cfg(target_pointer_width = "32")]
        sft: Box<[SFTRefStorage; ARCH_CHUNKS_32]>,
        #[cfg(target_pointer_width = "64")]
        sft: Box<[SFTRefStorage]>,
    }

    impl SFTSparseChunkMap {
        pub fn new() -> Self {
            #[cfg(target_pointer_width = "32")]
            {
                let vec: Vec<SFTRefStorage> = std::iter::repeat_with(SFTRefStorage::empty)
                    .take(ARCH_CHUNKS_32)
                    .collect();
                let sft: Box<[SFTRefStorage; ARCH_CHUNKS_32]> = vec
                    .into_boxed_slice()
                    .try_into()
                    .unwrap_or_else(|_| unreachable!());
                Self { sft }
            }
            #[cfg(target_pointer_width = "64")]
            {
                let sft: Vec<SFTRefStorage> = std::iter::repeat_with(SFTRefStorage::empty)
                    .take(vm_layout().max_chunks())
                    .collect();
                Self {
                    sft: sft.into_boxed_slice(),
                }
            }
        }

        #[inline(always)]
        pub(super) fn sft_slice(&self) -> &[SFTRefStorage] {
            &self.sft[..]
        }

        #[inline(always)]
        pub fn has_sft_entry(&self, addr: Address) -> bool {
            addr.chunk_index() < self.sft_slice().len()
        }

        #[inline(always)]
        pub fn get_side_metadata(&self) -> Option<&'static SideMetadataSpec> {
            None
        }

        #[inline(always)]
        pub fn get_checked(&self, address: Address) -> &'static dyn SFT {
            #[cfg(target_pointer_width = "32")]
            {
                // Every 32-bit `Address` has `address.chunk_index() == (u32 >> 22) < 1024`.
                self.sft[Fin::<ARCH_CHUNKS_32>::from_pow2_mask(address.chunk_index())].load()
            }
            #[cfg(target_pointer_width = "64")]
            {
                if let Some(cell) = self.sft.get(address.chunk_index()) {
                    cell.load()
                } else {
                    &EMPTY_SPACE_SFT
                }
            }
        }

        pub fn update_static(
            &self,
            space: &'static (dyn SFT + Sync + 'static),
            start: Address,
            bytes: usize,
        ) {
            if DEBUG_SFT {
                self.log_update(space, start, bytes);
            }
            if bytes == 0 {
                return;
            }
            assert!(
                start.as_usize().checked_add(bytes).is_some(),
                "SFTSparseChunkMap::update start + bytes overflowed Address"
            );
            let first = start.chunk_index();
            let last = (start + (bytes - 1)).chunk_index();
            let slice = self.sft_slice();
            assert!(
                last < slice.len(),
                "SFTSparseChunkMap::update chunk range {first}..={last} exceeds table length {}",
                slice.len()
            );
            for (offset, cell) in slice[first..=last].iter().enumerate() {
                if cfg!(debug_assertions) {
                    let old = cell.load().name();
                    let new = space.name();
                    debug_assert!(
                        old == EMPTY_SFT_NAME || new == EMPTY_SFT_NAME || old == new,
                        "attempt to overwrite a non-empty chunk {} ({}) in SFT map (from {} to {})",
                        first + offset,
                        chunk_index_to_address(first + offset),
                        old,
                        new
                    );
                }
                let _ = offset;
                cell.store_static(space);
            }
            if DEBUG_SFT {
                self.trace_sft_map();
            }
        }

        pub fn clear_checked(&self, chunk_start: Address) {
            if DEBUG_SFT {
                debug!(
                    "Clear SFT for chunk {} (was {})",
                    chunk_start,
                    self.get_checked(chunk_start).name()
                );
            }
            assert!(chunk_start.is_aligned_to(BYTES_IN_CHUNK));
            let chunk_idx = chunk_start.chunk_index();
            if let Some(cell) = self.sft_slice().get(chunk_idx) {
                cell.store_static(&EMPTY_SPACE_SFT);
            }
        }

        fn log_update(&self, space: &(dyn SFT + Sync + 'static), start: Address, bytes: usize) {
            debug!("Update SFT for Chunk {} as {}", start, space.name());
            let first = start.chunk_index();
            let start_chunk = chunk_index_to_address(first);
            debug!(
                "Update SFT for {} bytes of Chunk {} #{}",
                bytes, start_chunk, first
            );
        }

        fn trace_sft_map(&self) {
            trace!("{}", self.print_sft_map());
        }

        fn print_sft_map(&self) -> String {
            let mut res = String::new();
            let slice = self.sft_slice();
            const SPACE_PER_LINE: usize = 10;
            for i in (0..slice.len()).step_by(SPACE_PER_LINE) {
                let max = (i + SPACE_PER_LINE).min(slice.len());
                let space_names: Vec<&str> = slice[i..max]
                    .iter()
                    .map(|cell| cell.load().name())
                    .collect();
                res.push_str(&format!(
                    "{}: {}\n",
                    chunk_index_to_address(i),
                    space_names.join(",")
                ));
            }
            res
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::heap::layout::vm_layout::{VMLayout, BYTES_IN_CHUNK};

    #[test]
    fn vm_layout_32bit_max_chunks_does_not_overflow() {
        let layout32 = VMLayout::new_32bit();
        assert_eq!(layout32.max_chunks(), 1024);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn space_map_zero_bytes_is_noop_and_valid_update_roundtrips() {
        let map = space_map::SFTSpaceMap::new();
        map.update_static(&EMPTY_SPACE_SFT, Address::MAX, 0);

        let extent = 1usize << crate::util::heap::layout::vm_layout::vm_layout().log_space_extent;
        let space1_start = crate::util::heap::layout::vm_layout::vm_layout().heap_start;
        assert_eq!(space1_start.as_usize(), extent);
        assert!(map.has_sft_entry(space1_start));
        map.update_static(&EMPTY_SPACE_SFT, space1_start, BYTES_IN_CHUNK);
        assert_eq!(map.get_checked(space1_start).name(), EMPTY_SFT_NAME);
        map.clear_checked(space1_start);
    }

    #[test]
    fn global_sft_map_out_of_bounds_get_checked_returns_empty_sft() {
        assert_eq!(
            crate::mmtk::SFT_MAP.get_checked(Address::MAX).name(),
            EMPTY_SFT_NAME
        );
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    #[should_panic(expected = "SFTSpaceMap::update start + bytes overflowed Address")]
    fn space_map_update_rejects_address_overflow() {
        let map = space_map::SFTSpaceMap::new();
        map.update_static(&EMPTY_SPACE_SFT, Address::MAX, BYTES_IN_CHUNK);
    }

    #[test]
    fn sparse_chunk_map_zero_bytes_is_noop_and_valid_update_roundtrips() {
        let map = sparse_chunk_map::SFTSparseChunkMap::new();
        map.update_static(&EMPTY_SPACE_SFT, Address::MAX, 0);

        let chunk0 = Address::ZERO;
        assert!(map.has_sft_entry(chunk0));
        map.update_static(&EMPTY_SPACE_SFT, chunk0, BYTES_IN_CHUNK);
        assert_eq!(map.get_checked(chunk0).name(), EMPTY_SFT_NAME);
        map.clear_checked(chunk0);
    }

    #[test]
    #[should_panic(expected = "SFTSparseChunkMap::update start + bytes overflowed Address")]
    fn sparse_chunk_map_update_rejects_address_overflow() {
        let map = sparse_chunk_map::SFTSparseChunkMap::new();
        map.update_static(&EMPTY_SPACE_SFT, Address::MAX, BYTES_IN_CHUNK);
    }

    #[test]
    fn dense_chunk_map_rejects_out_of_bounds_contiguous_lookup_and_zero_bytes() {
        crate::util::test_util::serial_test(|| {
            crate::util::metadata::side_metadata::core_test_initialize_side_metadata();
            let map = dense_chunk_map::SFTDenseChunkMap::new();
            map.notify_space_creation(&EMPTY_SPACE_SFT);
            map.update_static(&EMPTY_SPACE_SFT, Address::MAX, 0);
            assert!(!map.has_sft_entry(Address::MAX));
            assert_eq!(map.get_checked(Address::MAX).name(), EMPTY_SFT_NAME);
            map.clear_checked(Address::MAX);
        });
    }
}
