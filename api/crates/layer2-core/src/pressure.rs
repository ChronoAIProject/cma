//! Policy and state for sustained resource pressure.

use serde::{Deserialize, Serialize};

/// When a session's memory counts as being under pressure, and when it stops.
///
/// The constructor refuses combinations that would turn a sustained signal
/// into repeated crossings. Fields stay private because their invariants are
/// relational rather than properties of any one number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "PressurePolicyWire", try_from = "PressurePolicyWire")]
pub struct MemoryPressurePolicy {
    enter_permille: u16,
    exit_permille: u16,
    enter_samples: u16,
    exit_samples: u16,
    settle_seconds: u32,
}

impl MemoryPressurePolicy {
    /// The narrowest accepted hysteresis band, in permille.
    const MIN_BAND: u16 = 50;
    /// The shortest accepted floor beneath a close, in seconds.
    const MIN_SETTLE: u32 = 60;

    pub fn new(
        enter_permille: u16,
        exit_permille: u16,
        enter_samples: u16,
        exit_samples: u16,
        settle_seconds: u32,
    ) -> Result<Self, InvalidPressurePolicy> {
        if !(1..=1000).contains(&enter_permille) {
            return Err(InvalidPressurePolicy::ThresholdOutOfRange {
                got: enter_permille,
            });
        }
        let width = enter_permille.saturating_sub(exit_permille);
        if width < Self::MIN_BAND {
            return Err(InvalidPressurePolicy::BandTooNarrow {
                enter: enter_permille,
                exit: exit_permille,
                width,
                min: Self::MIN_BAND,
            });
        }
        if enter_samples < 2 {
            return Err(InvalidPressurePolicy::EntryIsACrossing { got: enter_samples });
        }
        if exit_samples < enter_samples {
            return Err(InvalidPressurePolicy::LeavingIsEasierThanEntering {
                enter: enter_samples,
                exit: exit_samples,
            });
        }
        if settle_seconds < Self::MIN_SETTLE {
            return Err(InvalidPressurePolicy::SettleTooShort {
                got: settle_seconds,
                min: Self::MIN_SETTLE,
            });
        }
        Ok(Self {
            enter_permille,
            exit_permille,
            enter_samples,
            exit_samples,
            settle_seconds,
        })
    }

    pub fn enter_permille(self) -> u16 {
        self.enter_permille
    }

    pub fn exit_permille(self) -> u16 {
        self.exit_permille
    }

    pub fn enter_samples(self) -> u16 {
        self.enter_samples
    }

    pub fn exit_samples(self) -> u16 {
        self.exit_samples
    }

    pub fn settle_seconds(self) -> u32 {
        self.settle_seconds
    }
}

impl Default for MemoryPressurePolicy {
    /// The shipped signal requires five entry samples, fifteen exit samples,
    /// and a 120-second floor. At a two-second cadence an episode therefore
    /// takes at least 160 seconds to open, settle, and close.
    fn default() -> Self {
        Self::new(800, 700, 5, 15, 120).expect("the shipped pressure policy is valid")
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct PressurePolicyWire {
    enter_permille: u16,
    exit_permille: u16,
    enter_samples: u16,
    exit_samples: u16,
    settle_seconds: u32,
}

impl From<MemoryPressurePolicy> for PressurePolicyWire {
    fn from(value: MemoryPressurePolicy) -> Self {
        Self {
            enter_permille: value.enter_permille,
            exit_permille: value.exit_permille,
            enter_samples: value.enter_samples,
            exit_samples: value.exit_samples,
            settle_seconds: value.settle_seconds,
        }
    }
}

impl TryFrom<PressurePolicyWire> for MemoryPressurePolicy {
    type Error = InvalidPressurePolicy;

    fn try_from(value: PressurePolicyWire) -> Result<Self, Self::Error> {
        Self::new(
            value.enter_permille,
            value.exit_permille,
            value.enter_samples,
            value.exit_samples,
            value.settle_seconds,
        )
    }
}

/// A memory-pressure policy was refused, and which rule refused it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidPressurePolicy {
    #[error("the entry threshold must be between 1 and 1000 permille, got {got}")]
    ThresholdOutOfRange { got: u16 },
    #[error(
        "the hysteresis band must be at least {min} permille wide; {enter} over {exit} is {width}"
    )]
    BandTooNarrow {
        enter: u16,
        exit: u16,
        width: u16,
        min: u16,
    },
    #[error("entering takes at least 2 consecutive samples, got {got}")]
    EntryIsACrossing { got: u16 },
    #[error("leaving must take at least as many samples as entering; {exit} against {enter}")]
    LeavingIsEasierThanEntering { enter: u16, exit: u16 },
    #[error("the settle floor must be at least {min}s, got {got}s")]
    SettleTooShort { got: u32, min: u32 },
}
