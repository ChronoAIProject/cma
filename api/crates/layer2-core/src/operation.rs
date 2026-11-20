//! Durable idempotency claims shared by side-effecting public operations.

use std::time::{Duration, SystemTime};

use crate::SubjectId;

/// Shared public operation-key grammar: one to 255 visible ASCII bytes.
pub fn valid_idempotency_key(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.len() <= 255 && bytes.iter().all(|byte| (0x21..=0x7e).contains(byte))
}

/// Failure returned by a durable foundation store.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// The store could not be reached.
    #[error("store unavailable: {0}")]
    Unavailable(String),
    /// Durable bytes did not satisfy their contract.
    #[error("stored data not usable: {0}")]
    Malformed(String),
    /// Another writer changed the record after it was read.
    #[error("stored revision changed: expected {expected}, found {actual}")]
    RevisionConflict { expected: u64, actual: u64 },
}

/// One subject-scoped attempt to perform a named operation once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationClaim {
    claim_id: String,
    subject: SubjectId,
    operation: String,
    key: String,
    fingerprint: [u8; 32],
    lease_expires_at: SystemTime,
    outcome_retention: Duration,
}

impl OperationClaim {
    /// Build a durable operation claim.
    pub fn new(
        subject: SubjectId,
        operation: impl Into<String>,
        key: impl Into<String>,
        fingerprint: [u8; 32],
        lease_expires_at: SystemTime,
        outcome_retention: Duration,
    ) -> Self {
        Self {
            claim_id: uuid::Uuid::new_v4().simple().to_string(),
            subject,
            operation: operation.into(),
            key: key.into(),
            fingerprint,
            lease_expires_at,
            outcome_retention,
        }
    }

    /// Unique generation owned only by the process that acquired this claim.
    pub fn claim_id(&self) -> &str {
        &self.claim_id
    }

    /// The subject whose operation this is.
    pub fn subject(&self) -> &SubjectId {
        &self.subject
    }

    /// The interface-defined operation name.
    pub fn operation(&self) -> &str {
        &self.operation
    }

    /// The caller-supplied operation key.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// A digest of everything that distinguishes the request.
    pub fn fingerprint(&self) -> &[u8; 32] {
        &self.fingerprint
    }

    /// The finite ownership window for the process executing this claim.
    pub fn lease_expires_at(&self) -> SystemTime {
        self.lease_expires_at
    }

    /// How long an exact completed outcome remains replayable.
    pub fn outcome_retention(&self) -> Duration {
        self.outcome_retention
    }
}

/// What an atomic attempt to reserve an operation key found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationReservation {
    Acquired,
    Pending,
    RecoveryRequired,
    Complete(Vec<u8>),
    FingerprintMismatch,
}

/// Durable, subject-scoped operation claims and opaque replay outcomes.
#[async_trait::async_trait]
pub trait OperationStore: Send + Sync {
    async fn reserve(&self, claim: &OperationClaim) -> Result<OperationReservation, StoreError>;
    async fn complete(&self, claim: &OperationClaim, outcome: Vec<u8>) -> Result<(), StoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_keys_share_the_public_visible_ascii_contract() {
        for (key, accepted) in [
            (b"retry-1".to_vec(), true),
            (b"".to_vec(), false),
            (b"contains space".to_vec(), false),
            (b"line\nfeed".to_vec(), false),
            (b"\xff".to_vec(), false),
            (vec![b'x'; 255], true),
            (vec![b'x'; 256], false),
        ] {
            assert_eq!(valid_idempotency_key(&key), accepted);
        }
    }

    #[test]
    fn a_claim_preserves_its_durable_identity() {
        let deadline = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        let claim = OperationClaim::new(
            SubjectId::new("subject-a"),
            "create-resource",
            "attempt-a",
            [9; 32],
            deadline,
            Duration::from_secs(86_400),
        );
        assert_eq!(claim.subject().as_str(), "subject-a");
        assert_eq!(claim.operation(), "create-resource");
        assert_eq!(claim.key(), "attempt-a");
        assert_eq!(claim.fingerprint(), &[9; 32]);
        assert!(!claim.claim_id().is_empty());
        assert_eq!(claim.lease_expires_at(), deadline);
    }
}
