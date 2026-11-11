//! The platform admission state and spend read model, independent of storage.

use super::{BudgetError, Closure, ClosureReason, PlatformPeriodLedger, SpendLevels};

/// The closures on the current period row: one mechanism with several reasons.
/// An unconfigured platform (no enabled policy) is open.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlatformAdmissionState {
    pub configured: bool,
    pub closures: Vec<Closure>,
}

impl PlatformAdmissionState {
    pub fn closed_for_commitments(&self) -> bool {
        self.commitment_refusal().is_some()
    }

    pub fn closed_for_registration(&self) -> bool {
        !self.closures.is_empty()
    }

    /// The typed refusal for a new commitment: exhaustion wins over staleness.
    pub fn commitment_refusal(&self) -> Option<BudgetError> {
        let has = |reason| self.closures.iter().any(|closure| closure.reason == reason);
        if has(ClosureReason::SpendStop) {
            Some(BudgetError::PlatformExhausted)
        } else if has(ClosureReason::MeteringUnavailable) {
            Some(BudgetError::Unavailable)
        } else {
            None
        }
    }
}

/// The operator's view of the current period. Values are private accounting
/// units and never leave the authenticated admin surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformSpendState {
    pub settings_revision: u64,
    pub levels: Option<SpendLevels>,
    pub committed: Option<u64>,
    pub ledger: Option<PlatformPeriodLedger>,
}

/// A replaceable platform spend boundary. Reads never open a transaction.
#[async_trait::async_trait]
pub trait PlatformSpendLedger: Send + Sync {
    async fn admission_state(&self, now: u64) -> Result<PlatformAdmissionState, BudgetError>;
    async fn spend_state(&self, now: u64) -> Result<PlatformSpendState, BudgetError>;
    /// Append an immutable observation and update the period row together.
    /// An identical repeat is idempotent; a different value for the same
    /// instant is a conflict.
    async fn record_cloud_observation(
        &self,
        input: CloudObservationInput,
        now: u64,
    ) -> Result<PlatformSpendState, ObservationError>;
}

/// One operator observation of the platform's cloud spend from the period start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloudObservationInput {
    pub observed_through: u64,
    pub units: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ObservationError {
    /// Outside the current period, in the future, unsafe, or no enabled period.
    #[error("observation is outside the current platform period")]
    Invalid,
    /// The same instant was already recorded with a different value.
    #[error("observation conflicts with a recorded value")]
    Conflict,
    #[error("platform spend accounting is unavailable")]
    Unavailable,
}
