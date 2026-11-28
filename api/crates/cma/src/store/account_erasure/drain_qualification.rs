use super::{CohortBlock, Error};

pub(super) const MAX_LIFETIME_SECONDS: u64 = 3_600;
pub(super) const MAX_DRAIN_SECONDS: u64 = 86_400;

/// Limits from an independently verified writer and transaction qualification.
///
/// The caller must bind the evidence to this database and every writer, prove
/// that new references observe the erasure barrier, and maintain the qualified
/// limits throughout cleanup. The finality bound includes transaction cleanup,
/// acknowledgement and clock uncertainty; a server TTL alone is insufficient.
/// Configuration and a readable Mongo parameter cannot mint this authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TransactionDrainQualification {
    evidence_digest: [u8; 32],
    maximum_lifetime_seconds: u64,
    finality_bound_seconds: u64,
    execution_erasure_evidence: Option<[u8; 32]>,
}

impl TransactionDrainQualification {
    pub fn from_verified_limits(
        evidence_digest: [u8; 32],
        maximum_lifetime_seconds: u64,
        finality_bound_seconds: u64,
    ) -> Result<Self, Error> {
        if evidence_digest == [0; 32] {
            return Err(Error::cohort(
                CohortBlock::TransactionDrainQualificationRequired,
            ));
        }
        if maximum_lifetime_seconds == 0 || maximum_lifetime_seconds > MAX_LIFETIME_SECONDS {
            return Err(invalid(
                "qualified transaction lifetime",
                MAX_LIFETIME_SECONDS,
            ));
        }
        if finality_bound_seconds < maximum_lifetime_seconds
            || finality_bound_seconds > MAX_DRAIN_SECONDS
        {
            return Err(invalid("qualified transaction finality", MAX_DRAIN_SECONDS));
        }
        Ok(Self {
            evidence_digest,
            maximum_lifetime_seconds,
            finality_bound_seconds,
            execution_erasure_evidence: None,
        })
    }

    /// Bind an independent receipt confirming compatible serving and rollback
    /// readers, drained incompatible writers, and a reconciled legacy cohort.
    pub fn with_execution_erasure(mut self, evidence_digest: [u8; 32]) -> Result<Self, Error> {
        if evidence_digest == [0; 32] {
            return Err(Error::cohort(
                CohortBlock::ExecutionErasureQualificationRequired,
            ));
        }
        self.execution_erasure_evidence = Some(evidence_digest);
        Ok(self)
    }

    pub(super) fn allows_execution_erasure(self) -> bool {
        self.execution_erasure_evidence.is_some()
    }

    pub(super) fn seconds(
        self,
        observed_lifetime: Option<u64>,
        minimum: u64,
    ) -> Result<u64, Error> {
        if observed_lifetime
            .is_some_and(|seconds| seconds == 0 || seconds > self.maximum_lifetime_seconds)
        {
            return Err(invalid(
                "transactionLifetimeLimitSeconds",
                self.maximum_lifetime_seconds,
            ));
        }
        if minimum > MAX_DRAIN_SECONDS {
            return Err(invalid(
                "CHRONO_ACCOUNT_ERASURE_TRANSACTION_DRAIN_SECONDS",
                MAX_DRAIN_SECONDS,
            ));
        }
        Ok(self.finality_bound_seconds.max(minimum))
    }

    #[cfg(test)]
    pub(super) fn fixture() -> Self {
        Self::from_verified_limits([1; 32], 60, 120).unwrap()
    }
}

pub(super) fn invalid(parameter: &'static str, maximum_seconds: u64) -> Error {
    Error::cohort(CohortBlock::InvalidTransactionDrain {
        parameter,
        maximum_seconds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_qualification_is_required_even_for_a_readable_lifetime() {
        assert!(TransactionDrainQualification::from_verified_limits([0; 32], 60, 120).is_err());
        assert!(TransactionDrainQualification::from_verified_limits([1; 32], 0, 120).is_err());
        assert!(TransactionDrainQualification::from_verified_limits([1; 32], 60, 59).is_err());
        assert!(TransactionDrainQualification::from_verified_limits([1; 32], 3601, 86400).is_err());
        assert!(TransactionDrainQualification::from_verified_limits([1; 32], 60, 86401).is_err());
    }

    #[test]
    fn execution_erasure_requires_separate_evidence() {
        let authority = TransactionDrainQualification::fixture();
        assert!(!authority.allows_execution_erasure());
        assert!(authority.with_execution_erasure([0; 32]).is_err());
        assert!(authority
            .with_execution_erasure([2; 32])
            .unwrap()
            .allows_execution_erasure());
    }

    #[test]
    fn observed_lifetime_cannot_exceed_the_qualified_writer_bound() {
        let authority = TransactionDrainQualification::fixture();
        assert_eq!(authority.seconds(Some(60), 0).unwrap(), 120);
        assert_eq!(authority.seconds(None, 0).unwrap(), 120);
        assert_eq!(authority.seconds(Some(60), 240).unwrap(), 240);
        assert!(authority.seconds(Some(61), 120).is_err());
        assert!(authority.seconds(Some(0), 120).is_err());
        assert!(authority.seconds(None, u64::MAX).is_err());
    }
}
