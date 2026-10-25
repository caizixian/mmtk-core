// ANCHOR: imports
use super::MyGC; // Add
use crate::MMTK;
use crate::plan::mutator_context::Mutator;
use crate::plan::mutator_context::MutatorBuilder;
use crate::plan::mutator_context::MutatorConfig;
use crate::plan::AllocationSemantics;
use crate::util::alloc::allocators::AllocatorSelector;
use crate::util::alloc::BumpAllocator;
use crate::util::opaque_pointer::*;
use crate::vm::VMBinding;
use crate::plan::mutator_context::{
    create_allocator_mapping, create_space_mapping, ReservedAllocators,
};
use enum_map::EnumMap;
// Remove crate::plan::mygc::MyGC
// Remove mygc_mutator_noop
// ANCHOR_END: imports

// Add
pub fn mygc_mutator_prepare<VM: VMBinding>(
    _mutator: &mut Mutator<VM>,
    _tls: VMWorkerThread,
) {
    // Do nothing
}

// Add
// ANCHOR: release
pub fn mygc_mutator_release<VM: VMBinding>(
    mutator: &mut Mutator<VM>,
    _tls: VMWorkerThread,
) {
    let tospace = mutator
        .plan
        .downcast_ref::<MyGC<VM>>()
        .unwrap()
        .tospace();
    // rebind the allocation bump pointer to the appropriate semispace
    let bump_allocator = mutator
        .allocator_downcast_mut_for_semantic::<BumpAllocator<VM>>(AllocationSemantics::Default);
    bump_allocator.rebind(tospace);
}
// ANCHOR_END: release

// Modify
// ANCHOR: allocator_mapping
const RESERVED_ALLOCATORS: ReservedAllocators = ReservedAllocators {
    n_bump_pointer: 1,
    ..ReservedAllocators::DEFAULT
};

lazy_static! {
    pub static ref ALLOCATOR_MAPPING: EnumMap<AllocationSemantics, AllocatorSelector> = {
        let mut map = create_allocator_mapping(RESERVED_ALLOCATORS, true);
        map[AllocationSemantics::Default] = AllocatorSelector::BumpPointer(0);
        map
    };
}
// ANCHOR_END: allocator_mapping

pub fn create_mygc_mutator<VM: VMBinding>(
    mutator_tls: VMMutatorThread,
    mmtk: &'static MMTK<VM>,
) -> Mutator<VM> {
    // ANCHOR: plan_downcast
    let mygc = mmtk.get_plan().downcast_ref::<MyGC<VM>>().unwrap();
    // ANCHOR_END: plan_downcast
    let config = MutatorConfig {
        allocator_mapping: &ALLOCATOR_MAPPING,
        // Modify
        // ANCHOR: space_mapping
        space_mapping: Box::new({
            let mut vec = create_space_mapping(RESERVED_ALLOCATORS, true, mygc);
            vec.push((AllocatorSelector::BumpPointer(0), mygc.tospace()));
            vec
        }),
        // ANCHOR_END: space_mapping
        prepare_func: &mygc_mutator_prepare, // Modify
        release_func: &mygc_mutator_release, // Modify
    };

    let builder = MutatorBuilder::new(mutator_tls, mmtk, config);
    builder.build()
}
