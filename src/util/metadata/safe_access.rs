use crate::util::Address;
use crate::util::metadata::metadata_val_traits::MetadataValue;
use crate::util::metadata::side_metadata::SideMetadataSpec;
use std::marker::PhantomData;
use std::sync::atomic::Ordering;

/// Proof that the world is stopped. Only the GC controller can create this.
/// This allows safe non-atomic access to metadata.
#[derive(Copy, Clone)]
pub struct StwProof(());

impl StwProof {
    /// Only callable by the GC controller after stopping mutators.
    ///
    /// # Safety
    /// The caller must ensure that the world is stopped and no concurrent access to metadata is possible.
    pub unsafe fn new() -> Self {
        StwProof(())
    }

    /// Create a new `StwProof` for tests.
    #[cfg(test)]
    pub fn new_for_tests() -> Self {
        StwProof(())
    }
}

/// A safe wrapper around a metadata address.
/// It is tied to the lifetime of the `SideMetadataSpec` that created it.
pub struct MetadataSlot<'a, T: MetadataValue> {
    addr: Address,
    _spec: PhantomData<&'a SideMetadataSpec>,
    _marker: PhantomData<T>,
}

impl<'a, T: MetadataValue> MetadataSlot<'a, T> {
    /// Create a new `MetadataSlot` from a raw address.
    ///
    /// # Safety
    /// The caller must ensure the address is valid for the spec and type `T`.
    pub unsafe fn new_unchecked(addr: Address) -> Self {
        Self {
            addr,
            _spec: PhantomData,
            _marker: PhantomData,
        }
    }

    /// Load the value from the slot.
    pub fn load(&self) -> T {
        unsafe { T::load(self.addr) }
    }

    /// Atomic load from the slot.
    pub fn load_atomic(&self, order: Ordering) -> T {
        unsafe { T::load_atomic(self.addr, order) }
    }

    /// Store a value to the slot.
    pub fn store(&self, value: T) {
        unsafe { T::store(self.addr, value) }
    }

    /// Atomic store to the slot.
    pub fn store_atomic(&self, value: T, order: Ordering) {
        unsafe { T::store_atomic(self.addr, value, order) }
    }

    /// Compare and exchange the value in the slot.
    pub fn compare_exchange(
        &self,
        current: T,
        new: T,
        success: Ordering,
        failure: Ordering,
    ) -> Result<T, T> {
        unsafe { T::compare_exchange(self.addr, current, new, success, failure) }
    }

    /// Fetch and add.
    pub fn fetch_add(&self, value: T, order: Ordering) -> T {
        unsafe { T::fetch_add(self.addr, value, order) }
    }

    /// Fetch and sub.
    pub fn fetch_sub(&self, value: T, order: Ordering) -> T {
        unsafe { T::fetch_sub(self.addr, value, order) }
    }

    /// Fetch and and.
    pub fn fetch_and(&self, value: T, order: Ordering) -> T {
        unsafe { T::fetch_and(self.addr, value, order) }
    }

    /// Fetch and or.
    pub fn fetch_or(&self, value: T, order: Ordering) -> T {
        unsafe { T::fetch_or(self.addr, value, order) }
    }

    /// Fetch and update.
    pub fn fetch_update<F>(
        &self,
        set_order: Ordering,
        fetch_order: Ordering,
        f: F,
    ) -> Result<T, T>
    where
        F: FnMut(T) -> Option<T>,
    {
        unsafe { T::fetch_update(self.addr, set_order, fetch_order, f) }
    }
}
