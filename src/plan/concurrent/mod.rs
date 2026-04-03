pub mod barrier;
pub(super) mod concurrent_marking_work;
pub(super) mod global;

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
}

impl Pause {
    pub fn to_u8(pause: Option<Pause>) -> u8 {
        pause.map(|p| p as u8).unwrap_or(0)
    }

    pub fn from_u8(val: u8) -> Option<Pause> {
        match val {
            0 => None,
            1 => Some(Pause::Full),
            2 => Some(Pause::InitialMark),
            3 => Some(Pause::FinalMark),
            _ => panic!("Invalid Pause value: {}", val),
        }
    }
}
