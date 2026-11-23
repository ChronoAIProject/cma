//! Reader compatibility for a staged service-wide financial authority.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BillingMode {
    Legacy,
    NyxId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BillingEpoch(u64);

impl TryFrom<i64> for BillingEpoch {
    type Error = AuthorityError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        u64::try_from(value)
            .map(Self)
            .map_err(|_| AuthorityError::Malformed)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorityObservation {
    Absent,
    Retained {
        epoch: BillingEpoch,
        mode: BillingMode,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AuthorityError {
    #[error("invalid billing authority")]
    Malformed,
    #[error("billing authority requires a qualified execution protocol")]
    UnsupportedExecution,
}

impl AuthorityObservation {
    pub fn require_legacy_compatible(self) -> Result<(), AuthorityError> {
        match self {
            Self::Absent
            | Self::Retained {
                epoch: BillingEpoch(0),
                mode: BillingMode::Legacy,
            } => Ok(()),
            Self::Retained { .. } => Err(AuthorityError::UnsupportedExecution),
        }
    }
}

/// Reader compatibility never establishes participation by all financial writers.
pub const ACTIVATION_BLOCKERS: &[&str] = &[
    "all_role_membership_unavailable",
    "mutation_epoch_fencing_unavailable",
    "runtime_deadline_transition_unavailable",
    "model_and_platform_transition_unavailable",
    "legacy_obligation_reconciliation_unqualified",
    "model_routing_and_control_unqualified",
    "rollback_artifacts_unqualified",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_absent_or_prepared_legacy_has_execution_compatibility() {
        assert!(AuthorityObservation::Absent
            .require_legacy_compatible()
            .is_ok());
        for epoch in [0, 1, i64::MAX] {
            for mode in [BillingMode::Legacy, BillingMode::NyxId] {
                let value = AuthorityObservation::Retained {
                    epoch: epoch.try_into().unwrap(),
                    mode,
                };
                assert_eq!(
                    value.require_legacy_compatible().is_ok(),
                    epoch == 0 && mode == BillingMode::Legacy
                );
            }
        }
        assert!(BillingEpoch::try_from(-1).is_err());
        assert!(BillingEpoch::try_from(i64::MIN).is_err());
        assert!(ACTIVATION_BLOCKERS.contains(&"all_role_membership_unavailable"));
        assert!(ACTIVATION_BLOCKERS.contains(&"mutation_epoch_fencing_unavailable"));
    }
}
