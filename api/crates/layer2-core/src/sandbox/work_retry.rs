//! Durable attempt counts determine retry pacing without changing work records.
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkRetryClass {
    /// An admitted operation is waiting for initial readiness or reconnection.
    Interactive,
    /// Cleanup and maintenance retain pending work even after their budget ends.
    Maintenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkRetryFailure {
    Waiting,
    Transient,
    ExternalAction,
}

impl WorkRetryFailure {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Transient => "transient",
            Self::ExternalAction => "external_action",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkRetryDecision {
    pub delay: Duration,
    pub parked: bool,
}

pub const WORK_RETRY_PROBE_INTERVAL: Duration = Duration::from_secs(30 * 60);
pub const WORK_RETRY_BUDGET: u32 = 12;
pub const WORK_RETRY_EXTERNAL_BUDGET: u32 = 3;

impl WorkRetryFailure {
    pub const fn retry_budget(self) -> u32 {
        match self {
            Self::ExternalAction => WORK_RETRY_EXTERNAL_BUDGET,
            Self::Waiting | Self::Transient => WORK_RETRY_BUDGET,
        }
    }
}

/// `attempts` is the persisted count before publishing this failed attempt.
/// The caller retains the claim and records the chosen due time atomically.
/// Advancing or scheduling new work resets that count through the existing model.
pub fn work_retry_policy(
    class: WorkRetryClass,
    failure: WorkRetryFailure,
    attempts: u32,
    base: Duration,
) -> WorkRetryDecision {
    if class == WorkRetryClass::Maintenance && attempts >= failure.retry_budget() {
        return WorkRetryDecision {
            delay: WORK_RETRY_PROBE_INTERVAL.max(base),
            parked: true,
        };
    }
    // Preserve three initial interactive retries, then increase gradually.
    // Saturation also covers a record retained through very many failed probes.
    let exponent = match class {
        WorkRetryClass::Interactive => attempts.saturating_sub(2),
        WorkRetryClass::Maintenance => attempts,
    }
    .min(4);
    WorkRetryDecision {
        delay: base
            .saturating_mul(1_u32 << exponent)
            .min(Duration::from_secs(300).max(base)),
        parked: false,
    }
}

#[cfg(test)]
#[path = "work_retry_tests.rs"]
mod tests;
