// All functions here are extern function. There is no point for marking them as unsafe.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

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

#[no_mangle]
pub extern "C" fn mmtk_set_option_from_string(
    builder: Option<&mut MMTKBuilder>,
    name: *const c_char,
    value: *const c_char,
) -> bool {
    // SAFETY: The caller must ensure that `name` and `value` are valid null-terminated C strings.
    let builder = builder.expect("builder is null");
    let (name_str, value_str) = unsafe {
        (
            CStr::from_ptr(name),
            CStr::from_ptr(value),
        )
    };
    builder.set_option(name_str.to_str().unwrap(), value_str.to_str().unwrap())
}

#[no_mangle]
pub extern "C" fn mmtk_set_fixed_heap_size(builder: Option<&mut MMTKBuilder>, heap_size: usize) -> bool {
    let builder = builder.expect("builder is null");
    builder
        .options
        .gc_trigger
        .set(mmtk::util::options::GCTriggerSelector::FixedHeapSize(
            heap_size,
        ))
}

#[no_mangle]
pub fn mmtk_init(builder: Box<MMTKBuilder>) {

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

#[no_mangle]
pub extern "C" fn mmtk_destroy_mutator(mut mutator: Box<Mutator<DummyVM>>) {
    // notify mmtk-core about destroyed mutator
    // SAFETY: The caller must ensure that `mutator` is a valid pointer to a `Mutator`
    // that was created by `Box::into_raw` (e.g. by `mmtk_bind_mutator`).
    memory_manager::destroy_mutator(&mut *mutator);
}

#[no_mangle]
pub extern "C" fn mmtk_alloc(
    mutator: Option<&mut Mutator<DummyVM>>,
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
    let mutator = mutator.expect("mutator is null");
    memory_manager::alloc::<DummyVM>(mutator, size, align, offset, semantics)
}

#[no_mangle]
pub extern "C" fn mmtk_post_alloc(
    mutator: Option<&mut Mutator<DummyVM>>,
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
    let mutator = mutator.expect("mutator is null");
    memory_manager::post_alloc::<DummyVM>(mutator, refer, bytes, semantics)
}

#[no_mangle]
pub extern "C" fn mmtk_start_worker(tls: VMWorkerThread, worker: Box<GCWorker<DummyVM>>) {
    // SAFETY: The caller must ensure that `worker` is a valid pointer to a `GCWorker`
    // that was created by `Box::into_raw`.
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

#[cfg(feature = "is_mmtk_object")]
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
pub extern "C" fn mmtk_handle_user_collection_request(tls: VMMutatorThread) {
    memory_manager::handle_user_collection_request::<DummyVM>(mmtk(), tls);
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
        // We use safe Rust here instead of calling mmtk_create_builder to avoid unsafe in tests.
        let mut builder = mmtk::MMTKBuilder::new();

        // Set option by value using extern "C" wrapper.
        let success = mmtk_set_fixed_heap_size(Some(&mut builder), 1048576);
        assert!(success);

        // Set option by value.
        let name = CString::new("plan").unwrap();
        let val = CString::new("NoGC").unwrap();
        let success = mmtk_set_option_from_string(Some(&mut builder), name.as_ptr(), val.as_ptr());
        assert!(success);

        // Init MMTk
        mmtk_init(Box::new(builder));

        // Create an MMTk mutator
        let tls = VMMutatorThread(VMThread(OpaquePointer::UNINITIALIZED)); // FIXME: Use the actual thread pointer or identifier
        let mut mutator = memory_manager::bind_mutator(mmtk(), tls);

        // Do an allocation
        let addr = mmtk_alloc(Some(&mut mutator), 16, 8, 0, mmtk::AllocationSemantics::Default);
        assert!(!addr.is_zero());

        // Turn the allocation address into the object reference.
        let obj = DummyVM::object_start_to_ref(addr);

        // Post allocation
        mmtk_post_alloc(Some(&mut mutator), obj, 16, mmtk::AllocationSemantics::Default);

        // If the thread quits, destroy the mutator.
        memory_manager::destroy_mutator(&mut mutator);
    }
}
