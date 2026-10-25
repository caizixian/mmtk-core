#![deny(unsafe_op_in_unsafe_fn)]

use super::sft::*;
use crate::util::metadata::side_metadata::SideMetadataSpec;
use crate::util::Address;

use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::OnceLock;

/// SFTMap manages the SFT table, and mapping between addresses with indices in the table. The trait allows
/// us to have multiple implementations of the SFT table.
pub trait SFTMap: Send + Sync {
    /// Check if the address has an SFT entry in the map (including an empty SFT entry). This is mostly a bound check
    /// to make sure that we won't have an index-out-of-bound error. For the sake of performance, the implementation
    /// of other methods in this trait (such as get_unchecked(), update() and clear()) does not need to do this check implicitly.
    /// Instead, they assume the address has a valid entry in the SFT. If an address could be arbitary, they should call this
    /// method as a pre-check before they call those methods in the trait. We also provide a method `get_checked()` which includes
    /// this check, and will return an empty SFT if the address is out of bound.
    fn has_sft_entry(&self, addr: Address) -> bool;

    /// Get the side metadata spec this SFT map uses.
    fn get_side_metadata(&self) -> Option<&SideMetadataSpec>;

    /// Get SFT for the address. The address must have a valid SFT entry in the table (e.g. from an object reference, or from an address
    /// that is known to be in our spaces). Otherwise, use `get_checked()`.
    ///
    /// # Safety
    /// `self.has_sft_entry(address)` must hold (i.e. `address` maps to an in-bounds, mapped SFT
    /// table entry in `self`, such as an address from a valid `ObjectReference` or an allocated
    /// MMTk space range). Otherwise, call `has_sft_entry()` first or use `get_checked()`.
    unsafe fn get_unchecked(&self, address: Address) -> &dyn SFT;

    /// Get SFT for the address. The address can be arbitrary. For out-of-bound access, an empty SFT will be returned.
    /// We only provide the checked version for `get()`, as it may be used to query arbitrary objects and addresses. Other methods like `update/clear/etc` are
    /// mostly used inside MMTk, and in most cases, we know that they are within our space address range.
    fn get_checked(&self, address: Address) -> &dyn SFT;

    /// Set SFT for the address range. The address must have a valid SFT entry in the table.
    ///
    /// # Safety
    /// - `space` must be non-null, properly aligned, and valid to dereference as a shared
    ///   `&'static (dyn SFT + Sync + 'static)` for the entire lifetime of `self` (and for
    ///   `SFTDenseChunkMap`, `notify_space_creation(space)` must have been called beforehand).
    /// - `start + bytes` must not overflow `Address`, and every chunk/entry in `[start, start + bytes)`
    ///   must have a valid, mapped SFT entry in `self` (`has_sft_entry`).
    unsafe fn update(&self, space: SFTRawPointer, start: Address, bytes: usize);

    /// Notify the SFT map for space creation. `DenseChunkMap` needs to create an entry for the space.
    fn notify_space_creation(&mut self, _space: SFTRawPointer) {}

    /// Eagerly initialize the SFT table. For most implementations, it could be the same as update().
    /// However, we need this as a seprate method for SFTDenseChunkMap, as it needs to map side metadata first
    /// before setting the table.
    ///
    /// # Safety
    /// - `space` must be non-null, properly aligned, and valid to dereference as a shared
    ///   `&'static (dyn SFT + Sync + 'static)` for the entire lifetime of `self` (and for
    ///   `SFTDenseChunkMap`, `notify_space_creation(space)` must have been called beforehand).
    /// - `start + bytes` must not overflow `Address`, and every chunk/entry in `[start, start + bytes)`
    ///   must fall within `self`'s addressable SFT range (`has_sft_entry` once side metadata is mapped).
    unsafe fn eager_initialize(
        &mut self,
        space: *const (dyn SFT + Sync + 'static),
        start: Address,
        bytes: usize,
    ) {
        // SAFETY: Caller upholds all `update` preconditions for `(space, start, bytes)`.
        unsafe { self.update(space, start, bytes) };
    }

    /// Clear SFT for the address. The address must have a valid SFT entry in the table.
    ///
    /// # Safety
    /// - `self.has_sft_entry(address)` must hold (and for chunk-based maps, `address` must be
    ///   chunk-aligned when clearing a chunk entry).
    /// - No concurrent reader may call `get_unchecked` / `SFTTrace` on any address in the cleared
    ///   chunk or space until a valid SFT entry is installed again.
    unsafe fn clear(&self, address: Address);
}

/// Concrete enum dispatch for the global SFT map (`BP 3.2`), eliminating `Box<dyn SFTMap>` double
/// indirection and `InitializeOnce<T>` global mutable aliasing.
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

    /// Lookup `address` without bounds checking.
    ///
    /// # Safety
    /// `self.has_sft_entry(address)` must hold.
    #[inline(always)]
    pub(crate) unsafe fn get_unchecked(&self, address: Address) -> &'static dyn SFT {
        match self {
            Self::Empty => {
                debug_assert!(
                    false,
                    "SFTMapImpl::get_unchecked called on Empty fallback for {address}"
                );
                &EMPTY_SPACE_SFT
            }
            #[cfg(target_pointer_width = "64")]
            Self::Space(map) => map.get_unchecked_fast(address),
            // SAFETY: Caller guarantees `self.has_sft_entry(address)` holds, so
            // `SFTDenseChunkMap::is_index_mapped(address)` holds for `address`.
            Self::Dense(map) => unsafe { map.get_unchecked(address) },
            // SAFETY: Caller guarantees `self.has_sft_entry(address)` holds, so
            // `address.chunk_index()` is in bounds of `map`.
            Self::Sparse(map) => unsafe { map.get_unchecked(address) },
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
static SFT_MAP_PTR: AtomicPtr<SFTMapImpl> =
    AtomicPtr::new((&EMPTY_SFT_MAP_FALLBACK as *const SFTMapImpl).cast_mut());

/// Initialize the global SFT map once and publish its `'static` pointer with `Release` ordering.
pub(crate) fn ensure_sft_map_initialized() -> &'static SFTMapImpl {
    let ptr = SFT_MAP_PTR.load(Ordering::Acquire);
    if !std::ptr::eq(ptr, &EMPTY_SFT_MAP_FALLBACK) {
        // SAFETY: Any non-fallback pointer stored in `SFT_MAP_PTR` was published with `Release`
        // from `&'static SFTMapImpl` owned by `SFT_MAP_OWNER` and is never borrowed as `&mut` or
        // dropped.
        return unsafe { &*ptr };
    }
    let map = SFT_MAP_OWNER.get_or_init(|| {
        SFTRefStorage::pre_use_check();
        create_sft_map_impl()
    });
    SFT_MAP_PTR.store((map as *const SFTMapImpl).cast_mut(), Ordering::Release);
    map
}

/// Zero-sized global handle for lock-free SFT dispatch (`crate::mmtk::SFT_MAP`).
pub struct GlobalSftMap;

impl GlobalSftMap {
    #[inline(always)]
    fn map_acquire(&self) -> &'static SFTMapImpl {
        let ptr = SFT_MAP_PTR.load(Ordering::Acquire);
        // SAFETY: `SFT_MAP_PTR` starts at `&EMPTY_SFT_MAP_FALLBACK` and is only updated in
        // `ensure_sft_map_initialized` via `Ordering::Release` to point to the `'static`
        // `SFTMapImpl` inside `SFT_MAP_OWNER` (`OnceLock<SFTMapImpl>`). Pairing `Ordering::Acquire`
        // here with that `Release` store guarantees `ptr` is non-null, aligned, and points to a
        // fully initialized `'static` `SFTMapImpl` that is never borrowed as `&mut` or dropped.
        unsafe { &*ptr }
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

    /// Lookup `address` on the GC tracing hotpath without `OnceLock` state checks or bounds checks.
    ///
    /// # Safety
    /// - `ensure_sft_map_initialized` (run during `MMTK::new` -> `Plan::initialize_sft`) must have
    ///   happened-before this call on the calling thread (which holds on all GC worker threads
    ///   spawned after `MMTK::new` and mutators bound after `MMTK::new`).
    /// - `self.has_sft_entry(address)` must hold.
    #[inline(always)]
    pub unsafe fn get_unchecked(&self, address: Address) -> &'static dyn SFT {
        let ptr = SFT_MAP_PTR.load(Ordering::Relaxed);
        // SAFETY: By caller precondition, `ensure_sft_map_initialized()` already happened-before
        // this call on the calling thread, so a `Relaxed` load on `SFT_MAP_PTR` observes the
        // initialized `'static` `SFTMapImpl` (including on weakly-ordered architectures) without
        // `OnceLock` branches, and `self.has_sft_entry(address)` satisfies `get_unchecked`.
        unsafe { (&*ptr).get_unchecked(address) }
    }

    /// Register a `'static` space in `SFT_MAP` safely (`0` `unsafe`).
    #[allow(dead_code)]
    pub fn notify_space_creation_static(&self, space: &'static (dyn SFT + Sync + 'static)) {
        ensure_sft_map_initialized().notify_space_creation_raw(space);
    }

    /// Eagerly initialize SFT entries (and dense-chunk side metadata if needed) for a `'static` space.
    #[allow(dead_code)]
    pub fn eager_initialize_static(
        &self,
        space: &'static (dyn SFT + Sync + 'static),
        start: Address,
        bytes: usize,
    ) {
        ensure_sft_map_initialized().eager_initialize_static(space, start, bytes);
    }

    /// Update SFT entries for `[start, start + bytes)` with a `'static` space reference.
    #[allow(dead_code)]
    pub fn update_static(
        &self,
        space: &'static (dyn SFT + Sync + 'static),
        start: Address,
        bytes: usize,
    ) {
        self.map_acquire().update_static(space, start, bytes);
    }

    /// Safely clear the SFT entry for `address` if it is mapped.
    #[allow(dead_code)]
    pub fn clear_checked(&self, address: Address) {
        self.map_acquire().clear_checked(address);
    }

    /// Set SFT for `[start, start + bytes)` from a raw `'static` trait-object pointer.
    ///
    /// # Safety
    /// - `space` must be non-null, properly aligned, and valid to dereference as a shared
    ///   `&'static (dyn SFT + Sync + 'static)` for `'static` (and registered via
    ///   `notify_space_creation` when `SFTDenseChunkMap` is active).
    /// - `start + bytes` must not overflow `Address`, and every chunk/entry in `[start, start + bytes)`
    ///   must satisfy `self.has_sft_entry(...)`.
    pub unsafe fn update(
        &self,
        space: *const (dyn SFT + Sync + 'static),
        start: Address,
        bytes: usize,
    ) {
        // SAFETY: Caller guarantees `space` is non-null, aligned, and valid as a `'static` shared
        // `dyn SFT + Sync` reference for the lifetime of the process.
        let space_static: &'static (dyn SFT + Sync + 'static) = unsafe { &*space };
        self.map_acquire().update_static(space_static, start, bytes);
    }

    /// Clear SFT for `address`.
    ///
    /// # Safety
    /// - `self.has_sft_entry(address)` must hold (and `address` must be chunk-aligned for chunk maps).
    /// - No concurrent reader may call `get_unchecked` / `SFTTrace` on any address in the cleared
    ///   chunk or space until a valid SFT entry is installed again.
    pub unsafe fn clear(&self, address: Address) {
        debug_assert!(self.has_sft_entry(address));
        self.map_acquire().clear_checked(address);
    }
}

/// Stack-local adapter passed as `&mut dyn SFTMap` to [`crate::policy::space::Space::initialize_sft`],
/// delegating to `&'static SFTMapImpl` via `&self` without ever forming `&mut` to global state.
pub(crate) struct SftMapInitHandle(&'static SFTMapImpl);

impl SftMapInitHandle {
    pub(crate) fn new(map: &'static SFTMapImpl) -> Self {
        Self(map)
    }
}

impl SFTMap for SftMapInitHandle {
    fn has_sft_entry(&self, addr: Address) -> bool {
        self.0.has_sft_entry(addr)
    }

    fn get_side_metadata(&self) -> Option<&SideMetadataSpec> {
        self.0.get_side_metadata()
    }

    unsafe fn get_unchecked(&self, address: Address) -> &dyn SFT {
        // SAFETY: Caller upholds `self.has_sft_entry(address)`.
        unsafe { self.0.get_unchecked(address) }
    }

    fn get_checked(&self, address: Address) -> &dyn SFT {
        self.0.get_checked(address)
    }

    unsafe fn update(&self, space: SFTRawPointer, start: Address, bytes: usize) {
        // SAFETY: Caller guarantees `space` is non-null, aligned, and valid as a `'static` shared
        // `dyn SFT + Sync` reference.
        let space_static: &'static (dyn SFT + Sync + 'static) = unsafe { &*space };
        self.0.update_static(space_static, start, bytes);
    }

    fn notify_space_creation(&mut self, space: SFTRawPointer) {
        self.0.notify_space_creation_raw(space);
    }

    unsafe fn eager_initialize(
        &mut self,
        space: *const (dyn SFT + Sync + 'static),
        start: Address,
        bytes: usize,
    ) {
        // SAFETY: Caller guarantees `space` is non-null, aligned, and valid as a `'static` shared
        // `dyn SFT + Sync` reference.
        let space_static: &'static (dyn SFT + Sync + 'static) = unsafe { &*space };
        self.0.eager_initialize_static(space_static, start, bytes);
    }

    unsafe fn clear(&self, address: Address) {
        debug_assert!(self.0.has_sft_entry(address));
        self.0.clear_checked(address);
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

/// The type we store SFT raw pointer as. It basically just double word sized atomic integer.
/// This type provides an abstraction so we can access SFT easily.
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

    /// Create an `SFTRefStorage` initialized with a `'static` SFT reference.
    pub fn from_static(sft: &'static (dyn SFT + Sync + 'static)) -> Self {
        let ptr: SFTRawPointer = sft;
        // SAFETY: `DoubleWord` has the exact same size as `SFTRawPointer` (verified by compile-time
        // assertion above) and has no invalid bit patterns; `sft` is a valid `'static` `SFT + Sync`
        // reference, establishing `SFTRefStorage`'s invariant.
        let val: DoubleWord = unsafe { std::mem::transmute(ptr) };
        Self(AtomicDoubleWord::new(val))
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
        let ptr: SFTRawPointer = sft;
        // SAFETY: `DoubleWord` has the exact same size as `SFTRawPointer` (compile-time asserted)
        // and `sft` is a valid `'static` `SFT + Sync` reference, preserving `SFTRefStorage`'s invariant.
        let val: DoubleWord = unsafe { std::mem::transmute(ptr) };
        self.0.store(val, Ordering::Release)
    }
}

impl std::default::Default for SFTRefStorage {
    fn default() -> Self {
        Self::from_static(&EMPTY_SPACE_SFT)
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
                sft: std::array::from_fn(|_| SFTRefStorage::default()),
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

        #[inline(always)]
        pub fn get_unchecked_fast(&self, address: Address) -> &'static dyn SFT {
            self.sft[self.addr_to_index(address)].load()
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
            assert!(i != 0, "Invalid index: there is no space for index 0");
            let start = Address::ZERO.add(i << log_space_extent);
            let extent = 1usize << log_space_extent;
            (start, start.add(extent))
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
        ///Synchronizes cold space registration (`notify_space_creation`) without `&mut SFTDenseChunkMap`.
        registry: Mutex<DenseIndexRegistry>,
    }

    impl SFTDenseChunkMap {
        const EMPTY_SFT_INDEX: u8 = 0;
        const MAX_SPACES: usize = u8::MAX as usize + 1;

        pub fn new() -> Self {
            Self {
                sft: std::array::from_fn(|_| SFTRefStorage::default()),
                published_count: AtomicUsize::new(1),
                registry: Mutex::new(DenseIndexRegistry {
                    next_index: 1,
                    index_map: HashMap::new(),
                }),
            }
        }

        #[inline(always)]
        fn is_index_mapped(addr: Address) -> bool {
            crate::util::metadata::side_metadata::address_to_meta_address(
                &SFT_DENSE_CHUNK_MAP_INDEX,
                addr,
            ) < SFT_DENSE_CHUNK_MAP_INDEX.upper_bound_address_for_contiguous()
                && SFT_DENSE_CHUNK_MAP_INDEX.is_mapped(addr)
        }

        #[inline]
        pub fn has_sft_entry(&self, addr: Address) -> bool {
            if Self::is_index_mapped(addr) {
                // SAFETY: `Self::is_index_mapped(addr)` verified above.
                let index = unsafe { Self::addr_to_index(addr) } as usize;
                index < self.published_count.load(Ordering::Acquire)
            } else {
                false
            }
        }

        #[inline]
        pub fn get_side_metadata(&self) -> Option<&'static SideMetadataSpec> {
            Some(&SFT_DENSE_CHUNK_MAP_INDEX)
        }

        #[inline(always)]
        pub fn get_checked(&self, address: Address) -> &'static dyn SFT {
            if Self::is_index_mapped(address) {
                // SAFETY: `Self::is_index_mapped(address)` verified above.
                let index = unsafe { Self::addr_to_index(address) } as usize;
                if index < self.published_count.load(Ordering::Acquire) {
                    self.sft[index].load()
                } else {
                    &EMPTY_SPACE_SFT
                }
            } else {
                &EMPTY_SPACE_SFT
            }
        }

        /// Fast-path lookup in `self.sft` (`u8 as usize < 256` elides bounds checks).
        ///
        /// # Safety
        /// `Self::is_index_mapped(address)` must hold (which holds whenever
        /// `self.has_sft_entry(address)` is true).
        #[inline(always)]
        pub unsafe fn get_unchecked(&self, address: Address) -> &'static dyn SFT {
            // SAFETY: Caller guarantees `Self::is_index_mapped(address)` holds.
            let index = unsafe { Self::addr_to_index(address) } as usize;
            self.sft[index].load()
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

        /// Read the 1-byte dense SFT index from `SFT_DENSE_CHUNK_MAP_INDEX` for `addr`.
        ///
        /// # Safety
        /// `Self::is_index_mapped(addr)` must hold so `addr` maps within the contiguous bounds of
        /// `SFT_DENSE_CHUNK_MAP_INDEX` and its side-metadata page is mapped.
        #[inline(always)]
        unsafe fn addr_to_index(addr: Address) -> u8 {
            // `Acquire` pairs with the `SeqCst` store in `update_static()` so any thread observing
            // a newly published non-zero `index` also observes the preceding `store_static(space)`
            // write to `self.sft[index]` on weakly-ordered architectures.
            SFT_DENSE_CHUNK_MAP_INDEX.load_atomic::<u8>(addr, Ordering::Acquire)
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
                let vec: Vec<SFTRefStorage> = std::iter::repeat_with(SFTRefStorage::default)
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
                let sft: Vec<SFTRefStorage> = std::iter::repeat_with(SFTRefStorage::default)
                    .take(vm_layout().max_chunks())
                    .collect();
                Self {
                    sft: sft.into_boxed_slice(),
                }
            }
        }

        #[inline(always)]
        fn sft_slice(&self) -> &[SFTRefStorage] {
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

        /// Lookup `address` assuming `self.has_sft_entry(address)` holds.
        ///
        /// # Safety
        /// `self.has_sft_entry(address)` must hold (`address.chunk_index() < self.sft.len()`).
        #[inline(always)]
        pub unsafe fn get_unchecked(&self, address: Address) -> &'static dyn SFT {
            #[cfg(target_pointer_width = "32")]
            {
                self.sft[Fin::<ARCH_CHUNKS_32>::from_pow2_mask(address.chunk_index())].load()
            }
            #[cfg(target_pointer_width = "64")]
            {
                // SAFETY: Caller guarantees `self.has_sft_entry(address)`, i.e.
                // `address.chunk_index() < self.sft.len()`.
                unsafe { self.sft.get_unchecked(address.chunk_index()) }.load()
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
        let space1_start = Address::ZERO + extent;
        assert!(map.has_sft_entry(space1_start));
        map.update_static(&EMPTY_SPACE_SFT, space1_start, BYTES_IN_CHUNK);
        assert_eq!(map.get_checked(space1_start).name(), EMPTY_SFT_NAME);
        assert_eq!(map.get_unchecked_fast(space1_start).name(), EMPTY_SFT_NAME);
        map.clear_checked(space1_start);
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
