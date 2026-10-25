use super::barrier::LXRFieldBarrierSemantics;
use super::LXR;
use crate::plan::barriers::FieldBarrier;
use crate::plan::mutator_context::create_allocator_mapping;
use crate::plan::mutator_context::create_space_mapping;
use crate::plan::mutator_context::Mutator;
use crate::plan::mutator_context::MutatorBuilder;
use crate::plan::mutator_context::MutatorConfig;
use crate::plan::mutator_context::ReservedAllocators;
use crate::plan::AllocationSemantics;
use crate::util::alloc::allocators::AllocatorSelector;
use crate::util::alloc::ImmixAllocator;
use crate::util::opaque_pointer::{VMMutatorThread, VMWorkerThread};
use crate::vm::VMBinding;
use crate::MMTK;
use enum_map::EnumMap;

pub fn lxr_mutator_prepare<VM: VMBinding>(mutator: &mut Mutator<VM>, _tls: VMWorkerThread) {
    let immix_allocator = mutator
        .allocator_downcast_mut_for_semantic::<ImmixAllocator<VM>>(AllocationSemantics::Default);
    immix_allocator.reset();
}

pub fn lxr_mutator_release<VM: VMBinding>(mutator: &mut Mutator<VM>, _tls: VMWorkerThread) {
    let immix_allocator = mutator
        .allocator_downcast_mut_for_semantic::<ImmixAllocator<VM>>(AllocationSemantics::Default);
    immix_allocator.reset();
}

const RESERVED_ALLOCATORS: ReservedAllocators = ReservedAllocators {
    n_immix: 1,
    ..ReservedAllocators::DEFAULT
};

lazy_static! {
    pub static ref ALLOCATOR_MAPPING: EnumMap<AllocationSemantics, AllocatorSelector> = {
        let mut map = create_allocator_mapping(RESERVED_ALLOCATORS, true);
        map[AllocationSemantics::Default] = AllocatorSelector::Immix(0);
        map
    };
}

pub fn create_lxr_mutator<VM: VMBinding>(
    mutator_tls: VMMutatorThread,
    mmtk: &'static MMTK<VM>,
) -> Mutator<VM> {
    let lxr = mmtk.get_plan().downcast_ref::<LXR<VM>>().unwrap();
    let config = MutatorConfig {
        allocator_mapping: &ALLOCATOR_MAPPING,
        space_mapping: Box::new({
            let mut vec = create_space_mapping(RESERVED_ALLOCATORS, true, mmtk.get_plan());
            vec.push((AllocatorSelector::Immix(0), &lxr.immix_space));
            vec
        }),
        prepare_func: &lxr_mutator_prepare,
        release_func: &lxr_mutator_release,
    };

    MutatorBuilder::new(mutator_tls, mmtk, config)
        .barrier(Box::new(FieldBarrier::new(LXRFieldBarrierSemantics::new(
            mmtk,
            mutator_tls,
        ))))
        .build()
}
