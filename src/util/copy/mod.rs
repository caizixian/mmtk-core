#![deny(unsafe_op_in_unsafe_fn)]

use std::sync::Arc;

use crate::plan::PlanConstraints;
use crate::policy::copy_context::PolicyCopyContext;
use crate::policy::copyspace::CopySpace;
use crate::policy::copyspace::CopySpaceCopyContext;
use crate::policy::immix::ImmixSpace;
use crate::policy::immix::{ImmixCopyContext, ImmixHybridCopyContext};
use crate::policy::space::Space;
use crate::util::object_forwarding;
use crate::util::opaque_pointer::VMWorkerThread;
use crate::util::{Address, ObjectReference};
use crate::vm::ObjectModel;
use crate::vm::VMBinding;
use crate::MMTK;

use enum_map::Enum;
use enum_map::EnumMap;

use super::alloc::allocator::AllocatorContext;

const MAX_COPYSPACE_COPY_ALLOCATORS: usize = 1;
const MAX_IMMIX_COPY_ALLOCATORS: usize = 1;
const MAX_IMMIX_HYBRID_COPY_ALLOCATORS: usize = 1;

type CopySpaceMapping<VM> = Vec<(CopySelector, &'static dyn Space<VM>)>;

/// A configuration for GCWorkerCopyContext.
/// Similar to a `MutatorConfig`,
/// We expect each copying plan to provide a CopyConfig.
pub struct CopyConfig<VM: VMBinding> {
    /// Mapping CopySemantics to the actual copying allocators (CopySelector)
    pub(crate) copy_mapping: EnumMap<CopySemantics, CopySelector>,
    /// Mapping copying allocators with space
    pub(crate) space_mapping: CopySpaceMapping<VM>,
    /// A reference to the plan constraints.
    /// GCWorkerCopyContext may have plan-specific behaviors dependson the plan constraints.
    pub(crate) constraints: &'static PlanConstraints,
}

impl<VM: VMBinding> Default for CopyConfig<VM> {
    fn default() -> Self {
        CopyConfig {
            copy_mapping: EnumMap::default(),
            space_mapping: vec![],
            constraints: &crate::plan::DEFAULT_PLAN_CONSTRAINTS,
        }
    }
}

/// The thread local struct for each GC worker for copying. Each GC worker should include
/// one instance of this struct for copying operations.
pub struct GCWorkerCopyContext<VM: VMBinding> {
    /// Copy allocator for `CopySpace` (niche-optimized `Option` via `&'static dyn Space` / `Arc`)
    copy: Option<CopySpaceCopyContext<VM>>,
    /// Copy allocator for `ImmixSpace` (niche-optimized `Option` via `Arc<AllocatorContext>`)
    immix: Option<ImmixCopyContext<VM>>,
    /// Hybrid copy allocator for `ImmixSpace` (niche-optimized `Option` via `Arc<AllocatorContext>`)
    immix_hybrid: Option<ImmixHybridCopyContext<VM>>,
    /// The config for the plan
    config: CopyConfig<VM>,
}

impl<VM: VMBinding> GCWorkerCopyContext<VM> {
    /// Rebind the `CopySpace` copy allocator at `index` to `space` for the current GC pause.
    pub(crate) fn rebind_copy_space(&mut self, index: usize, space: &'static CopySpace<VM>) {
        assert_eq!(
            index, 0,
            "CopySpaceCopyContext index out of bounds: {index}"
        );
        self.copy
            .as_mut()
            .expect("CopySpaceCopyContext slot is not initialized")
            .rebind(space);
    }

    /// Allocate for the object for GC copying.
    ///
    /// Arguments:
    /// * `original`: The original object that will be copied.
    /// * `bytes`: The size in bytes for the allocation.
    /// * `align`: The alignment in bytes for the allocation.
    /// * `offset`: The offset in bytes for the allocation.
    /// * `semantics`: The copy semantic for this coying allocation.
    ///   It determins which copy allocator will be used for the copying.
    pub fn alloc_copy(
        &mut self,
        original: ObjectReference,
        bytes: usize,
        align: usize,
        offset: usize,
        semantics: CopySemantics,
    ) -> Address {
        #[cfg(debug_assertions)]
        if bytes > self.config.constraints.max_non_los_default_alloc_bytes {
            warn!(
                "Attempted to copy an object of {} bytes (> {}) which should be allocated with LOS and not be copied.",
                bytes, self.config.constraints.max_non_los_default_alloc_bytes
            );
        }
        match self.config.copy_mapping[semantics] {
            CopySelector::CopySpace(_) => self
                .copy
                .as_mut()
                .expect("CopySpaceCopyContext slot is not initialized")
                .alloc_copy(original, bytes, align, offset),
            CopySelector::Immix(_) => self
                .immix
                .as_mut()
                .expect("ImmixCopyContext slot is not initialized")
                .alloc_copy(original, bytes, align, offset),
            CopySelector::ImmixHybrid(_) => self
                .immix_hybrid
                .as_mut()
                .expect("ImmixHybridCopyContext slot is not initialized")
                .alloc_copy(original, bytes, align, offset),
            CopySelector::Unused => unreachable!(),
        }
    }

    /// Post allocation after allocating an object.
    ///
    /// Arguments:
    /// * `object`: The newly allocated object (the new object after copying).
    /// * `bytes`: The size of the object in bytes.
    /// * `semantics`: The copy semantic used for the copying.
    pub fn post_copy(&mut self, object: ObjectReference, bytes: usize, semantics: CopySemantics) {
        if VM::VMObjectModel::LOCAL_FORWARDING_BITS_SPEC.is_in_header() {
            // Clear forwarding bits if the forwarding bits are in the header.
            object_forwarding::clear_forwarding_bits::<VM>(object);
        } else {
            // We ensure no stale side forwarding bits exist before tracing.
            debug_assert!(!object_forwarding::is_forwarded_or_being_forwarded::<VM>(
                object
            ));
        }
        // Policy specific post copy.
        match self.config.copy_mapping[semantics] {
            CopySelector::CopySpace(_) => self
                .copy
                .as_mut()
                .expect("CopySpaceCopyContext slot is not initialized")
                .post_copy(object, bytes),
            CopySelector::Immix(_) => self
                .immix
                .as_mut()
                .expect("ImmixCopyContext slot is not initialized")
                .post_copy(object, bytes),
            CopySelector::ImmixHybrid(_) => self
                .immix_hybrid
                .as_mut()
                .expect("ImmixHybridCopyContext slot is not initialized")
                .post_copy(object, bytes),
            CopySelector::Unused => unreachable!(),
        }
    }

    /// Prepare the copying allocators.
    pub fn prepare(&mut self) {
        // Delegate to prepare() for each policy copy context
        for (_, selector) in self.config.copy_mapping.iter() {
            match selector {
                CopySelector::CopySpace(_) => self
                    .copy
                    .as_mut()
                    .expect("CopySpaceCopyContext slot is not initialized")
                    .prepare(),
                CopySelector::Immix(_) => self
                    .immix
                    .as_mut()
                    .expect("ImmixCopyContext slot is not initialized")
                    .prepare(),
                CopySelector::ImmixHybrid(_) => self
                    .immix_hybrid
                    .as_mut()
                    .expect("ImmixHybridCopyContext slot is not initialized")
                    .prepare(),
                CopySelector::Unused => {}
            }
        }
    }

    /// Release the copying allocators.
    pub fn release(&mut self) {
        // Delegate to release() for each policy copy context
        for (_, selector) in self.config.copy_mapping.iter() {
            match selector {
                CopySelector::CopySpace(_) => self
                    .copy
                    .as_mut()
                    .expect("CopySpaceCopyContext slot is not initialized")
                    .release(),
                CopySelector::Immix(_) => self
                    .immix
                    .as_mut()
                    .expect("ImmixCopyContext slot is not initialized")
                    .release(),
                CopySelector::ImmixHybrid(_) => self
                    .immix_hybrid
                    .as_mut()
                    .expect("ImmixHybridCopyContext slot is not initialized")
                    .release(),
                CopySelector::Unused => {}
            }
        }
    }

    /// Create a GCWorkerCopyContext based on the configuration for a copying plan.
    ///
    /// Arguments:
    /// * `worker_tls`: The worker thread for this copy context.
    /// * `mmtk`: A reference to the current MMTK instance.
    /// * `config`: The configuration for the copy context.
    pub fn new(worker_tls: VMWorkerThread, mmtk: &MMTK<VM>, config: CopyConfig<VM>) -> Self {
        const _: () = assert!(MAX_COPYSPACE_COPY_ALLOCATORS == 1);
        const _: () = assert!(MAX_IMMIX_COPY_ALLOCATORS == 1);
        const _: () = assert!(MAX_IMMIX_HYBRID_COPY_ALLOCATORS == 1);
        let mut ret = GCWorkerCopyContext {
            copy: None,
            immix: None,
            immix_hybrid: None,
            config,
        };
        let context = Arc::new(AllocatorContext::new(mmtk));
        let mut copy_init = [false; MAX_COPYSPACE_COPY_ALLOCATORS];
        let mut immix_init = [false; MAX_IMMIX_COPY_ALLOCATORS];
        let mut immix_hybrid_init = [false; MAX_IMMIX_HYBRID_COPY_ALLOCATORS];

        // Initiate the copy context for each policy based on the space mapping.
        for &(selector, space) in ret.config.space_mapping.iter() {
            match selector {
                CopySelector::CopySpace(index) => {
                    let idx = index as usize;
                    assert_eq!(idx, 0, "CopySpaceCopyContext index out of bounds: {idx}");
                    assert!(!copy_init[idx], "Duplicate selector {selector:?}");
                    ret.copy = Some(CopySpaceCopyContext::new(
                        worker_tls,
                        context.clone(),
                        space.downcast_ref::<CopySpace<VM>>().unwrap(),
                    ));
                    copy_init[idx] = true;
                }
                CopySelector::Immix(index) => {
                    let idx = index as usize;
                    assert_eq!(idx, 0, "ImmixCopyContext index out of bounds: {idx}");
                    assert!(!immix_init[idx], "Duplicate selector {selector:?}");
                    ret.immix = Some(ImmixCopyContext::new(
                        worker_tls,
                        context.clone(),
                        space.downcast_ref::<ImmixSpace<VM>>().unwrap(),
                    ));
                    immix_init[idx] = true;
                }
                CopySelector::ImmixHybrid(index) => {
                    let idx = index as usize;
                    assert_eq!(idx, 0, "ImmixHybridCopyContext index out of bounds: {idx}");
                    assert!(!immix_hybrid_init[idx], "Duplicate selector {selector:?}");
                    ret.immix_hybrid = Some(ImmixHybridCopyContext::new(
                        worker_tls,
                        context.clone(),
                        space.downcast_ref::<ImmixSpace<VM>>().unwrap(),
                    ));
                    immix_hybrid_init[idx] = true;
                }
                CopySelector::Unused => unreachable!(),
            }
        }

        // Enforce that every active selector in `copy_mapping` refers to an initialized slot.
        for (_, &selector) in ret.config.copy_mapping.iter() {
            let initialized = match selector {
                CopySelector::CopySpace(index) => {
                    copy_init.get(index as usize).copied().unwrap_or(false)
                }
                CopySelector::Immix(index) => {
                    immix_init.get(index as usize).copied().unwrap_or(false)
                }
                CopySelector::ImmixHybrid(index) => immix_hybrid_init
                    .get(index as usize)
                    .copied()
                    .unwrap_or(false),
                CopySelector::Unused => true,
            };
            assert!(
                initialized,
                "CopyConfig::copy_mapping references uninitialized selector {selector:?}"
            );
        }

        ret
    }

    /// Create a stub GCWorkerCopyContext for non copying plans.
    pub fn new_non_copy() -> Self {
        GCWorkerCopyContext {
            copy: None,
            immix: None,
            immix_hybrid: None,
            config: CopyConfig::default(),
        }
    }
}

/// CopySemantics describes the copying operation. It depends on
/// the kinds of GC, and the space. For example, in a mature/major GC in
/// a generational plan, the nursery should have `PromoteToMature` while
/// the mature space should have `Mature`.
/// This enum may be expanded in the future to describe more semantics.
#[derive(Clone, Copy, Enum, Debug)]
pub enum CopySemantics {
    /// The default copy behavior.
    DefaultCopy,
    /// Copy in nursery generation.
    Nursery,
    /// Promote an object from nursery to mature spaces.
    PromoteToMature,
    /// Copy in mature generation.
    Mature,
}

impl CopySemantics {
    /// Are we copying to a mature space?
    pub fn is_mature(&self) -> bool {
        matches!(self, CopySemantics::PromoteToMature | CopySemantics::Mature)
    }
}

#[repr(C, u8)]
#[derive(Copy, Clone, Debug, Default)]
pub(crate) enum CopySelector {
    CopySpace(u8),
    Immix(u8),
    ImmixHybrid(u8),
    #[default]
    Unused,
}
