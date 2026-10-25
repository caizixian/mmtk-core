use crate::mmtk;
use crate::DummyVM;
use crate::SINGLETON;
use libc::c_char;
use mmtk::memory_manager;
use mmtk::scheduler::GCWorker;
use mmtk::util::opaque_pointer::*;
use mmtk::util::{Address, ObjectReference};
use mmtk::AllocationSemantics;
use mmtk::MMTKBuilder;
use mmtk::Mutator;
use std::ffi::CStr;

// This file exposes MMTk Rust API to the native code. This is not an exhaustive list of all the APIs.
// Most commonly used APIs are listed in https://docs.mmtk.io/api/mmtk/memory_manager/index.html. The binding can expose them here.

#[no_mangle]
pub extern "C" fn mmtk_create_builder() -> *mut MMTKBuilder {
    Box::into_raw(Box::new(mmtk::MMTKBuilder::new()))
}

/// Set an MMTk option by C string name and value on `builder`.
///
/// # Safety
/// `builder` must be a valid, unaliased `*mut MMTKBuilder` from [`mmtk_create_builder`], and
/// `name`/`value` must be valid, unaliased NUL-terminated C strings live for this call.
#[no_mangle]
pub unsafe extern "C" fn mmtk_set_option_from_string(
    builder: *mut MMTKBuilder,
    name: *const c_char,
    value: *const c_char,
) -> bool {
    // SAFETY: Caller guarantees an exclusive `MMTKBuilder` and valid NUL-terminated C strings.
    let (builder, name_str, value_str) =
        unsafe { (&mut *builder, CStr::from_ptr(name), CStr::from_ptr(value)) };
    builder.set_option(name_str.to_str().unwrap(), value_str.to_str().unwrap())
}

/// Set a fixed heap size option on `builder`.
///
/// # Safety
/// `builder` must be a valid, exclusively borrowed `*mut MMTKBuilder` from [`mmtk_create_builder`].
#[no_mangle]
pub unsafe extern "C" fn mmtk_set_fixed_heap_size(
    builder: *mut MMTKBuilder,
    heap_size: usize,
) -> bool {
    // SAFETY: Caller guarantees `builder` is valid and exclusively borrowed for this call.
    let builder = unsafe { &mut *builder };
    builder
        .options
        .gc_trigger
        .set(mmtk::util::options::GCTriggerSelector::FixedHeapSize(
            heap_size,
        ))
}

/// Initialize the global MMTk singleton by consuming `builder`.
///
/// # Safety
/// `builder` must come from [`mmtk_create_builder`] (`Box::into_raw`) with unique ownership
/// transferred once to this call.
#[no_mangle]
pub unsafe extern "C" fn mmtk_init(builder: *mut MMTKBuilder) {
    // SAFETY: Caller transfers unique ownership of `builder` from `mmtk_create_builder`.
    let builder = unsafe { Box::from_raw(builder) };

    // Create MMTK instance.
    let mmtk = memory_manager::mmtk_init::<DummyVM>(&builder);

    // Set SINGLETON to the instance.
    SINGLETON.set(mmtk).unwrap_or_else(|_| {
        panic!("Failed to set SINGLETON");
    });
}

#[no_mangle]
pub extern "C" fn mmtk_bind_mutator(tls: VMMutatorThread) -> *mut Mutator<DummyVM> {
    Box::into_raw(memory_manager::bind_mutator(mmtk(), tls))
}

/// Destroy and reclaim a mutator previously created by [`mmtk_bind_mutator`].
///
/// # Safety
/// `mutator` must come from [`mmtk_bind_mutator`] (`Box::into_raw`), outside any GC pause, with
/// unique ownership transferred once to this call.
#[no_mangle]
pub unsafe extern "C" fn mmtk_destroy_mutator(mutator: *mut Mutator<DummyVM>) {
    // SAFETY: Caller transfers unique ownership of `mutator` from `mmtk_bind_mutator`.
    let mut mutator = unsafe { Box::from_raw(mutator) };
    // notify mmtk-core about destroyed mutator, then let `mutator` drop to reclaim memory
    memory_manager::destroy_mutator(&mut mutator);
}

/// Allocate `size` bytes from MMTk using `mutator`.
///
/// # Safety
/// After [`mmtk_init`], `mutator` must point to an active `Mutator<DummyVM>` from
/// [`mmtk_bind_mutator`] exclusively owned by the calling mutator thread for this call.
#[no_mangle]
pub unsafe extern "C" fn mmtk_alloc(
    mutator: *mut Mutator<DummyVM>,
    size: usize,
    align: usize,
    offset: usize,
    mut semantics: AllocationSemantics,
) -> Address {
    // This just demonstrates that the binding should check against `max_non_los_default_alloc_bytes` to allocate large objects.
    // In pratice, a binding may want to lift this code to somewhere in the runtime where the allocated bytes is constant so
    // they can statically know if a normal allocation or a large object allocation is needed.
    if size
        >= mmtk()
            .get_plan()
            .constraints()
            .max_non_los_default_alloc_bytes
    {
        semantics = AllocationSemantics::Los;
    }
    // SAFETY: Caller guarantees `mutator` is active and exclusively borrowed by this thread.
    let mutator = unsafe { &mut *mutator };
    memory_manager::alloc::<DummyVM>(mutator, size, align, offset, semantics)
}

/// Run MMTk post-allocation hooks on a newly allocated object `refer`.
///
/// # Safety
/// After [`mmtk_init`], `mutator` must be an active, exclusively borrowed `Mutator<DummyVM>` from
/// [`mmtk_bind_mutator`], and `refer` must be a freshly allocated object of `bytes` size.
#[no_mangle]
pub unsafe extern "C" fn mmtk_post_alloc(
    mutator: *mut Mutator<DummyVM>,
    refer: ObjectReference,
    bytes: usize,
    mut semantics: AllocationSemantics,
) {
    // This just demonstrates that the binding should check against `max_non_los_default_alloc_bytes` to allocate large objects.
    // In pratice, a binding may want to lift this code to somewhere in the runtime where the allocated bytes is constant so
    // they can statically know if a normal allocation or a large object allocation is needed.
    if bytes
        >= mmtk()
            .get_plan()
            .constraints()
            .max_non_los_default_alloc_bytes
    {
        semantics = AllocationSemantics::Los;
    }
    // SAFETY: Caller guarantees `mutator` is active and exclusively borrowed for this call.
    let mutator = unsafe { &mut *mutator };
    memory_manager::post_alloc::<DummyVM>(mutator, refer, bytes, semantics)
}

/// Start the main execution loop for a GC worker thread.
///
/// # Safety
/// `worker` must be a `Box::into_raw`-allocated `GCWorker<DummyVM>` for `tls` after [`mmtk_init`]
/// with unique ownership transferred once to this call.
#[no_mangle]
pub unsafe extern "C" fn mmtk_start_worker(tls: VMWorkerThread, worker: *mut GCWorker<DummyVM>) {
    // SAFETY: Caller transfers unique ownership of `worker` for `tls`.
    let worker = unsafe { Box::from_raw(worker) };
    memory_manager::start_worker::<DummyVM>(mmtk(), tls, worker)
}

#[no_mangle]
pub extern "C" fn mmtk_initialize_collection(tls: VMThread) {
    memory_manager::initialize_collection(mmtk(), tls)
}

#[no_mangle]
pub extern "C" fn mmtk_used_bytes() -> usize {
    memory_manager::used_bytes(mmtk())
}

#[no_mangle]
pub extern "C" fn mmtk_free_bytes() -> usize {
    memory_manager::free_bytes(mmtk())
}

#[no_mangle]
pub extern "C" fn mmtk_total_bytes() -> usize {
    memory_manager::total_bytes(mmtk())
}

#[no_mangle]
pub extern "C" fn mmtk_is_live_object(object: ObjectReference) -> bool {
    memory_manager::is_live_object(object)
}

#[no_mangle]
pub extern "C" fn mmtk_will_never_move(object: ObjectReference) -> bool {
    !object.is_movable()
}

#[cfg(feature = "vo_bit")]
#[no_mangle]
pub extern "C" fn mmtk_is_mmtk_object(addr: Address) -> bool {
    memory_manager::is_mmtk_object(addr).is_some()
}

#[no_mangle]
pub extern "C" fn mmtk_is_in_mmtk_spaces(object: ObjectReference) -> bool {
    memory_manager::is_in_mmtk_spaces(object)
}

#[no_mangle]
pub extern "C" fn mmtk_is_mapped_address(address: Address) -> bool {
    memory_manager::is_mapped_address(address)
}

#[no_mangle]
pub extern "C" fn mmtk_handle_user_collection_request(tls: VMMutatorThread, force: bool) {
    memory_manager::handle_user_collection_request::<DummyVM>(mmtk(), tls, force);
}

#[no_mangle]
pub extern "C" fn mmtk_add_weak_candidate(reff: ObjectReference) {
    memory_manager::add_weak_candidate(mmtk(), reff)
}

#[no_mangle]
pub extern "C" fn mmtk_add_soft_candidate(reff: ObjectReference) {
    memory_manager::add_soft_candidate(mmtk(), reff)
}

#[no_mangle]
pub extern "C" fn mmtk_add_phantom_candidate(reff: ObjectReference) {
    memory_manager::add_phantom_candidate(mmtk(), reff)
}

#[no_mangle]
pub extern "C" fn mmtk_harness_begin(tls: VMMutatorThread) {
    memory_manager::harness_begin(mmtk(), tls)
}

#[no_mangle]
pub extern "C" fn mmtk_harness_end() {
    memory_manager::harness_end(mmtk())
}

#[no_mangle]
pub extern "C" fn mmtk_starting_heap_address() -> Address {
    memory_manager::starting_heap_address()
}

#[no_mangle]
pub extern "C" fn mmtk_last_heap_address() -> Address {
    memory_manager::last_heap_address()
}

#[no_mangle]
#[cfg(feature = "malloc_counted_size")]
pub extern "C" fn mmtk_counted_malloc(size: usize) -> Address {
    memory_manager::counted_malloc::<DummyVM>(mmtk(), size)
}
#[no_mangle]
pub extern "C" fn mmtk_malloc(size: usize) -> Address {
    memory_manager::malloc(size)
}

#[no_mangle]
#[cfg(feature = "malloc_counted_size")]
pub extern "C" fn mmtk_counted_calloc(num: usize, size: usize) -> Address {
    memory_manager::counted_calloc::<DummyVM>(mmtk(), num, size)
}
#[no_mangle]
pub extern "C" fn mmtk_calloc(num: usize, size: usize) -> Address {
    memory_manager::calloc(num, size)
}

#[no_mangle]
#[cfg(feature = "malloc_counted_size")]
pub extern "C" fn mmtk_realloc_with_old_size(
    addr: Address,
    size: usize,
    old_size: usize,
) -> Address {
    memory_manager::realloc_with_old_size::<DummyVM>(mmtk(), addr, size, old_size)
}
#[no_mangle]
pub extern "C" fn mmtk_realloc(addr: Address, size: usize) -> Address {
    memory_manager::realloc(addr, size)
}

#[no_mangle]
#[cfg(feature = "malloc_counted_size")]
pub extern "C" fn mmtk_free_with_size(addr: Address, old_size: usize) {
    memory_manager::free_with_size::<DummyVM>(mmtk(), addr, old_size)
}
#[no_mangle]
pub extern "C" fn mmtk_free(addr: Address) {
    memory_manager::free(addr)
}

#[no_mangle]
#[cfg(feature = "malloc_counted_size")]
pub extern "C" fn mmtk_get_malloc_bytes() -> usize {
    memory_manager::get_malloc_bytes(mmtk())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn mmtk_init_test() {
        // We demonstrate the main workflow to initialize MMTk, create mutators and allocate objects.
        let builder = mmtk_create_builder();
        let name = CString::new("plan").unwrap();
        let val = CString::new("NoGC").unwrap();
        let tls = VMMutatorThread(VMThread(OpaquePointer::UNINITIALIZED)); // FIXME: Use the actual thread pointer or identifier

        // SAFETY: Single-threaded end-to-end FFI smoke test:
        // - `builder` comes from `mmtk_create_builder`, is configured exclusively with live
        //   NUL-terminated `CString` pointers `name`/`val`, and is consumed once by `mmtk_init`.
        // - `mutator` comes from `mmtk_bind_mutator` after `mmtk_init`, allocates and initializes
        //   a 16-byte object exclusively on this thread, and is consumed once by `mmtk_destroy_mutator`.
        unsafe {
            assert!(mmtk_set_fixed_heap_size(builder, 1048576));
            assert!(mmtk_set_option_from_string(
                builder,
                name.as_ptr(),
                val.as_ptr()
            ));
            mmtk_init(builder);

            let mutator = mmtk_bind_mutator(tls);
            let addr = mmtk_alloc(mutator, 16, 8, 0, mmtk::AllocationSemantics::Default);
            assert!(!addr.is_zero());

            let obj = DummyVM::object_start_to_ref(addr);
            mmtk_post_alloc(mutator, obj, 16, mmtk::AllocationSemantics::Default);
            mmtk_destroy_mutator(mutator);
        }
    }
}
