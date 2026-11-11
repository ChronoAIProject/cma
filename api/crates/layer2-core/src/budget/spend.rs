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
}
