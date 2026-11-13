//! The platform admission state and spend read model, independent of storage.

use super::{BudgetError, Closure, ClosureReason, PlatformPeriodLedger, SpendLevels};
use crate::admission::{AdmissionCause, AdmissionRefusal, OperatorStop};

/// Parsing control must not make admitted work depend on readable operator state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum OperatorAdmissionState {
    #[default]
    Open,
    Stopped(OperatorStop),
    Unreadable,
}

impl OperatorAdmissionState {
    pub fn is_open(&self) -> bool {
        matches!(self, Self::Open)
    }
}

impl From<Option<OperatorStop>> for OperatorAdmissionState {
    fn from(stop: Option<OperatorStop>) -> Self {
        stop.map(Self::Stopped).unwrap_or_default()
    }
}

/// Spend closures and the independent operator control share one admission view.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlatformAdmissionState {
    pub configured: bool,
    pub closures: Vec<Closure>,
    pub operator_control: OperatorAdmissionState,
}

impl PlatformAdmissionState {
    pub fn closed_for_commitments(&self) -> bool {
        self.commitment_refusal().is_some()
    }

    pub fn closed_for_registration(&self) -> bool {
        !self.operator_control.is_open() || !self.closures.is_empty()
    }

    /// The typed refusal for a new commitment: exhaustion wins over staleness.
    pub fn commitment_refusal(&self) -> Option<AdmissionRefusal> {
        self.commitment_refusal_for(AdmissionCause::Fresh)
    }

    /// Previously admitted work bypasses only the operator stop. Spend checks
    /// retain their existing precedence and continuation behavior.
    pub fn commitment_refusal_for(&self, cause: AdmissionCause) -> Option<AdmissionRefusal> {
        let has = |reason| self.closures.iter().any(|closure| closure.reason == reason);
        if has(ClosureReason::SpendStop) {
            Some(AdmissionRefusal::Budget(BudgetError::PlatformExhausted))
        } else if has(ClosureReason::MeteringUnavailable) {
            Some(AdmissionRefusal::Budget(BudgetError::Unavailable))
        } else if cause == AdmissionCause::Fresh && !self.operator_control.is_open() {
            Some(AdmissionRefusal::Paused)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    use crate::admission::{AdmissionCause, AdmissionRefusal, OperatorStop};

    fn stopped() -> PlatformAdmissionState {
        PlatformAdmissionState {
            operator_control: OperatorAdmissionState::Stopped(OperatorStop {
                since: 1,
                revision: 1,
                actor: crate::SubjectId::new("operator"),
                reason: "Contain new work".into(),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn operator_stop_closes_fresh_work_and_registration_without_metering() {
        let state = stopped();
        assert!(!state.configured);
        assert!(state.closed_for_registration());
        assert_eq!(state.commitment_refusal(), Some(AdmissionRefusal::Paused));
        assert_eq!(
            state.commitment_refusal_for(AdmissionCause::Continuation),
            None
        );
    }

    #[test]
    fn unreadable_control_refuses_only_fresh_work_and_registration() {
        let mut state = PlatformAdmissionState {
            operator_control: OperatorAdmissionState::Unreadable,
            ..Default::default()
        };
        assert!(state.closed_for_registration());
        assert_eq!(state.commitment_refusal(), Some(AdmissionRefusal::Paused));
        assert_eq!(
            state.commitment_refusal_for(AdmissionCause::Continuation),
            None
        );
        state.closures.push(Closure {
            reason: ClosureReason::SpendStop,
            since: 1,
            settings_revision: 1,
            committed_at_entry: 100,
        });
        for cause in [AdmissionCause::Fresh, AdmissionCause::Continuation] {
            assert_eq!(
                state.commitment_refusal_for(cause),
                Some(AdmissionRefusal::Budget(BudgetError::PlatformExhausted))
            );
        }
    }

    #[test]
    fn spend_stop_keeps_precedence_for_fresh_work_and_continuations() {
        let mut state = stopped();
        state.closures.push(Closure {
            reason: ClosureReason::SpendStop,
            since: 2,
            settings_revision: 1,
            committed_at_entry: 100,
        });
        for cause in [AdmissionCause::Fresh, AdmissionCause::Continuation] {
            assert_eq!(
                state.commitment_refusal_for(cause),
                Some(AdmissionRefusal::Budget(BudgetError::PlatformExhausted))
            );
        }
    }
}

/// The operator's view of the current period. Values are private accounting
/// units and never leave the authenticated admin surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformSpendState {
    /// Budgets enabled with a platform policy that has a cloud source; false
    /// means metering is off, not that it is unavailable.
    pub metering_enabled: bool,
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
