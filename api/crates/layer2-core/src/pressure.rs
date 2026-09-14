//! Policy and state for sustained resource pressure.

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[path = "pressure_abandon.rs"]
mod abandon;

use abandon::abandon_episode;

/// When a runtime's memory counts as being under pressure, and when it stops.
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

/// When a runtime's processor use counts as sustained pressure.
///
/// CPU and memory use the same validated hysteresis and dwell contract. The
/// alias keeps the settings independently named without duplicating a policy
/// whose invariants and wire form are identical.
pub type CpuPressurePolicy = MemoryPressurePolicy;

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

/// One reading, reduced to what the pressure fold needs and an edge carries.
#[derive(Debug, Clone)]
pub struct PressureSample {
    /// Occupancy of the limit in permille, when a limit was reported.
    pub occupancy_permille: Option<u16>,
    /// Bytes in use at this reading.
    pub used_bytes: u64,
    /// The limit the reading was measured against.
    pub limit_bytes: Option<u64>,
    /// When the reading was taken, monotonically.
    pub at: Instant,
}

/// One processor reading, reduced to what the pressure fold needs.
#[derive(Debug, Clone)]
pub struct CpuPressureSample {
    /// Occupancy of the reported processor limit, in permille.
    pub occupancy_permille: Option<u16>,
    /// Processor use over the reading interval, in thousandths of one core.
    pub used_millis: u32,
    /// The processor limit the reading was measured against.
    pub limit_millis: Option<u32>,
    /// When the reading was taken, monotonically.
    pub at: Instant,
}

/// A runtime's current memory-pressure fold.
///
/// This state belongs to a live entry and is never stored. Only the two edges
/// it produces belong in durable history.
#[derive(Debug, Default)]
pub struct MemoryPressure {
    phase: Phase,
}

/// A runtime's current processor-pressure fold.
///
/// Like memory pressure, this state is live-only. Only its opening and closing
/// edges belong in durable history.
#[derive(Debug, Default)]
pub struct CpuPressure {
    phase: Phase,
}

#[derive(Debug)]
enum Phase {
    /// No episode. `above` counts consecutive readings at or above entry.
    Quiet { above: u16 },
    /// An episode is open.
    Open {
        opened_at: Instant,
        limit: u64,
        peak_permille: u16,
        peak: u64,
        /// Consecutive readings strictly below exit.
        below: u16,
        /// Consecutive entry-level readings while a close is pending.
        returning: u16,
        /// Set when the exit dwell completes.
        settling_since: Option<Instant>,
    },
}

impl Default for Phase {
    fn default() -> Self {
        Self::Quiet { above: 0 }
    }
}

/// The only two durable edges the fold produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PressureTransition {
    Opened {
        used_bytes: u64,
        limit_bytes: u64,
        occupancy_permille: u16,
    },
    Closed {
        peak_bytes: u64,
        limit_bytes: u64,
        peak_permille: u16,
        duration_seconds: u32,
    },
}

/// The only two durable processor-pressure edges the fold produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CpuPressureTransition {
    Opened {
        used_millis: u32,
        limit_millis: u32,
        occupancy_permille: u16,
    },
    Closed {
        peak_millis: u32,
        limit_millis: u32,
        peak_permille: u16,
        duration_seconds: u32,
    },
}

#[derive(Debug, Clone, Copy)]
struct EpisodeSample {
    occupancy_permille: Option<u16>,
    used: u64,
    limit: Option<u64>,
    at: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EpisodeTransition {
    Opened {
        used: u64,
        limit: u64,
        occupancy_permille: u16,
    },
    Closed {
        peak: u64,
        limit: u64,
        peak_permille: u16,
        duration_seconds: u32,
    },
}

impl MemoryPressure {
    /// Fold one reading, returning only an episode edge.
    pub fn evaluate(
        &mut self,
        sample: &PressureSample,
        policy: &MemoryPressurePolicy,
    ) -> Option<PressureTransition> {
        evaluate_episode(
            &mut self.phase,
            &EpisodeSample {
                occupancy_permille: sample.occupancy_permille,
                used: sample.used_bytes,
                limit: sample.limit_bytes,
                at: sample.at,
            },
            policy,
        )
        .map(|transition| match transition {
            EpisodeTransition::Opened {
                used,
                limit,
                occupancy_permille,
            } => PressureTransition::Opened {
                used_bytes: used,
                limit_bytes: limit,
                occupancy_permille,
            },
            EpisodeTransition::Closed {
                peak,
                limit,
                peak_permille,
                duration_seconds,
            } => PressureTransition::Closed {
                peak_bytes: peak,
                limit_bytes: limit,
                peak_permille,
                duration_seconds,
            },
        })
    }

    /// Close an open episode immediately when its live entry stops serving.
    pub fn abandon(&mut self, at: Instant) -> Option<PressureTransition> {
        abandon_episode(&mut self.phase, at).map(|transition| match transition {
            EpisodeTransition::Opened { .. } => unreachable!("abandon only closes an episode"),
            EpisodeTransition::Closed {
                peak,
                limit,
                peak_permille,
                duration_seconds,
            } => PressureTransition::Closed {
                peak_bytes: peak,
                limit_bytes: limit,
                peak_permille,
                duration_seconds,
            },
        })
    }
}

impl CpuPressure {
    /// Fold one reading, returning only an episode edge.
    pub fn evaluate(
        &mut self,
        sample: &CpuPressureSample,
        policy: &CpuPressurePolicy,
    ) -> Option<CpuPressureTransition> {
        evaluate_episode(
            &mut self.phase,
            &EpisodeSample {
                occupancy_permille: sample.occupancy_permille,
                used: u64::from(sample.used_millis),
                limit: sample.limit_millis.map(u64::from),
                at: sample.at,
            },
            policy,
        )
        .map(cpu_transition)
    }

    /// Close an open episode immediately when its live entry stops serving.
    pub fn abandon(&mut self, at: Instant) -> Option<CpuPressureTransition> {
        abandon_episode(&mut self.phase, at).map(cpu_transition)
    }
}

fn cpu_transition(transition: EpisodeTransition) -> CpuPressureTransition {
    match transition {
        EpisodeTransition::Opened {
            used,
            limit,
            occupancy_permille,
        } => CpuPressureTransition::Opened {
            used_millis: used as u32,
            limit_millis: limit as u32,
            occupancy_permille,
        },
        EpisodeTransition::Closed {
            peak,
            limit,
            peak_permille,
            duration_seconds,
        } => CpuPressureTransition::Closed {
            peak_millis: peak as u32,
            limit_millis: limit as u32,
            peak_permille,
            duration_seconds,
        },
    }
}

#[allow(clippy::question_mark)] // Keep the absence rule explicit at the fold boundary.
fn evaluate_episode(
    phase: &mut Phase,
    sample: &EpisodeSample,
    policy: &MemoryPressurePolicy,
) -> Option<EpisodeTransition> {
    let Some(occupancy) = sample.occupancy_permille else {
        return None;
    };
    let sample_limit = sample.limit?;

    match phase {
        Phase::Quiet { above } => {
            if occupancy >= policy.enter_permille {
                *above = above.saturating_add(1);
            } else {
                *above = 0;
            }
            if *above < policy.enter_samples {
                return None;
            }

            *phase = Phase::Open {
                opened_at: sample.at,
                limit: sample_limit,
                peak_permille: occupancy,
                peak: sample.used,
                below: 0,
                returning: 0,
                settling_since: None,
            };
            Some(EpisodeTransition::Opened {
                used: sample.used,
                limit: sample_limit,
                occupancy_permille: occupancy,
            })
        }
        Phase::Open {
            opened_at,
            limit,
            peak_permille,
            peak,
            below,
            returning,
            settling_since,
        } => {
            *peak_permille = (*peak_permille).max(occupancy);
            *peak = (*peak).max(sample.used);

            if occupancy >= policy.enter_permille {
                if settling_since.is_some() {
                    *returning = returning.saturating_add(1);
                    if *returning >= policy.enter_samples {
                        *settling_since = None;
                        *returning = 0;
                        *below = 0;
                        return None;
                    }
                } else {
                    *returning = 0;
                }
            } else {
                *returning = 0;
            }

            if occupancy < policy.exit_permille {
                *below = below.saturating_add(1);
                if *below >= policy.exit_samples && settling_since.is_none() {
                    *settling_since = Some(sample.at);
                }
            } else {
                *below = 0;
            }

            let since = (*settling_since)?;
            if sample.at.duration_since(since)
                < Duration::from_secs(u64::from(policy.settle_seconds))
            {
                return None;
            }

            let transition = EpisodeTransition::Closed {
                peak: *peak,
                limit: *limit,
                peak_permille: *peak_permille,
                duration_seconds: sample.at.duration_since(*opened_at).as_secs() as u32,
            };
            *phase = Phase::Quiet { above: 0 };
            Some(transition)
        }
    }
}
