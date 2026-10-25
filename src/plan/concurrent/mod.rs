pub mod barrier;
pub(super) mod concurrent_marking_work;
pub(super) mod global;

pub use self::global::ConcurrentPlan;

pub mod immix;

use bytemuck::NoUninit;

/// The pause type for a concurrent GC phase.
// TODO: This is probably not be general enough for all the concurrent plans.
// TODO: We could consider moving this to specific plans later.
#[repr(u8)]
#[derive(Debug, PartialEq, Eq, Copy, Clone, NoUninit, Default)]
pub enum Pause {
    /// A whole GC (including root scanning, closure, releasing, etc.) happening in a single pause.
    ///
    /// Don't be confused with "full-heap" GC in generational collectors.  `Pause::Full` can also
    /// refer to a nursery GC that happens in a single pause.
    #[default]
    Full = 1,
    /// The initial pause before concurrent marking.
    InitialMark,
    /// The pause after concurrent marking.
    FinalMark,
    /// A reference-counting-only pause. Used by plans that combine reference counting with
    /// optional concurrent marking (e.g. LXR). Concurrent marking is NOT active during this
    /// pause — any in-flight concurrent work is postponed.
    RefCount,
}

/// Lock-free atomic storage for `Option<Pause>`, encoding `None` as `0` and `Some(pause)` as
/// `pause as u8` (`1..=4`) without relying on unsound `Pod` bit-pattern impls on `Option<Pause>`.
#[derive(Default)]
pub(in crate::plan) struct AtomicPause(std::sync::atomic::AtomicU8);

impl AtomicPause {
    pub(in crate::plan) const fn new(pause: Option<Pause>) -> Self {
        Self(std::sync::atomic::AtomicU8::new(Self::encode(pause)))
    }

    #[inline(always)]
    const fn encode(pause: Option<Pause>) -> u8 {
        match pause {
            None => 0,
            Some(p) => p as u8,
        }
    }

    #[inline(always)]
    fn decode(raw: u8) -> Option<Pause> {
        match raw {
            0 => None,
            1 => Some(Pause::Full),
            2 => Some(Pause::InitialMark),
            3 => Some(Pause::FinalMark),
            4 => Some(Pause::RefCount),
            _ => unreachable!("Invalid AtomicPause discriminant: {}", raw),
        }
    }

    #[inline(always)]
    pub(in crate::plan) fn load(&self, order: std::sync::atomic::Ordering) -> Option<Pause> {
        Self::decode(self.0.load(order))
    }

    #[inline(always)]
    pub(in crate::plan) fn store(&self, pause: Option<Pause>, order: std::sync::atomic::Ordering) {
        self.0.store(Self::encode(pause), order);
    }
}
