//! Private, bounded diagnostic projection; never format the compatibility error.

use layer2_core::{
    agent_ownership::OwnershipError,
    sandbox::store::{AgentWorkClaimDetail, AgentWorkClaimError, SandboxStoreError},
    AgentId,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};
use tokio::time::Instant;
use uuid::Uuid;

macro_rules! closed_codes {
    ($name:ident { $($variant:ident => $code:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
        pub(super) enum $name {
            $(#[serde(rename = $code)] $variant),+
        }
        impl $name {
            pub(super) const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $code),+ }
            }
        }
    };
}

closed_codes!(Phase {
    Discovery => "discovery", Execution => "execution", Reconciliation => "reconciliation"
});
closed_codes!(Origin {
    Discovery => "discovery", OwnershipAcquire => "ownership_acquire",
    AuthorityScope => "authority_scope", Claim => "claim", Execute => "execute",
    PostWorkRead => "post_work_read", ContinuationRead => "continuation_read",
    Handoff => "handoff", Reconciliation => "reconciliation", Unspecified => "unspecified"
});
closed_codes!(CauseClass {
    OwnershipStale => "ownership_stale", OwnershipUnavailable => "ownership_unavailable",
    OwnershipInvalid => "ownership_invalid", IncompatibleProtocol => "incompatible_protocol",
    StoreConflict => "store_conflict", StoreUnavailable => "store_unavailable",
    StoredDataInvalid => "stored_data_invalid", DomainRefusal => "domain_refusal",
    TaskCancelled => "task_cancelled", TaskPanicked => "task_panicked"
});
closed_codes!(DetailCode {
    NotApplicable => "not_applicable", Suppressed => "unstructured_detail_suppressed",
    DiscoveryDeadline => "discovery_deadline", OwnershipDeadline => "ownership_deadline",
    ReceiverRevocationUnconfirmed => "receiver_revocation_unconfirmed",
    ClaimRetryExhausted => "claim_retry_exhausted", ClaimReadFailed => "claim_read_failed",
    ClaimDriverFailed => "claim_driver_failed",
    ClaimPublicationCustodyFailed => "claim_publication_custody_failed"
});

pub(super) const DISCOVERY_DEADLINE: &str = "discovery deadline";

/// Metadata is retained by the scheduler outside the spawned result as well.
/// Only the continuation counter is shared, and only within this task attempt.
#[derive(Clone)]
pub(super) struct LifecycleAttempt {
    phase: Phase,
    id: Uuid,
    agent_hash: Option<String>,
    started: Instant,
    continuation: Arc<AtomicU8>,
}

impl LifecycleAttempt {
    pub(super) fn new(phase: Phase, agent: Option<&AgentId>) -> Self {
        Self {
            phase,
            id: Uuid::new_v4(),
            agent_hash: agent.map(|id| format!("{:x}", Sha256::digest(id.as_str().as_bytes()))),
            started: Instant::now(),
            continuation: Arc::new(AtomicU8::new(0)),
        }
    }

    pub(super) fn continuation(&self, index: u8) {
        self.continuation.store(index.min(6), Ordering::Relaxed);
    }

    pub(super) fn phase(&self) -> &'static str {
        self.phase.as_str()
    }

    fn observe(
        &self,
        origin: Origin,
        cause_class: CauseClass,
        detail_code: DetailCode,
    ) -> FailureObservation {
        FailureObservation {
            phase: self.phase,
            origin,
            cause_class,
            detail_code,
            detail_retained: !matches!(
                detail_code,
                DetailCode::NotApplicable | DetailCode::Suppressed
            ),
            detail_suppressed: detail_code == DetailCode::Suppressed,
            attempt_id: self.id,
            agent_hash: self.agent_hash.clone(),
            elapsed_ms: self.started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            continuation_index: self.continuation.load(Ordering::Relaxed),
        }
    }

    pub(super) fn store(&self, origin: Origin, error: SandboxStoreError) -> LifecycleTaskFailure {
        let (cause, detail) = match &error {
            SandboxStoreError::Conflict => (CauseClass::StoreConflict, DetailCode::NotApplicable),
            SandboxStoreError::Unavailable(text) => (
                CauseClass::StoreUnavailable,
                if origin == Origin::Discovery && text == DISCOVERY_DEADLINE {
                    DetailCode::DiscoveryDeadline
                } else {
                    DetailCode::Suppressed
                },
            ),
            SandboxStoreError::Corrupt(_) => {
                (CauseClass::StoredDataInvalid, DetailCode::Suppressed)
            }
            SandboxStoreError::InvalidTransaction(_) => {
                (CauseClass::DomainRefusal, DetailCode::Suppressed)
            }
            _ => (CauseClass::DomainRefusal, DetailCode::NotApplicable),
        };
        LifecycleTaskFailure {
            compatibility_error: error,
            observation: self.observe(origin, cause, detail),
        }
    }

    pub(super) fn claim(&self, error: AgentWorkClaimError) -> LifecycleTaskFailure {
        let detail = match (&error.store_error, error.detail) {
            (SandboxStoreError::Unavailable(_), Some(AgentWorkClaimDetail::RetryExhausted)) => {
                DetailCode::ClaimRetryExhausted
            }
            (SandboxStoreError::Unavailable(_), Some(AgentWorkClaimDetail::Read)) => {
                DetailCode::ClaimReadFailed
            }
            (SandboxStoreError::Unavailable(_), Some(AgentWorkClaimDetail::Driver)) => {
                DetailCode::ClaimDriverFailed
            }
            (SandboxStoreError::Unavailable(_), Some(AgentWorkClaimDetail::PublicationCustody)) => {
                DetailCode::ClaimPublicationCustodyFailed
            }
            _ => return self.store(Origin::Claim, error.store_error),
        };
        LifecycleTaskFailure {
            compatibility_error: error.store_error,
            observation: self.observe(Origin::Claim, CauseClass::StoreUnavailable, detail),
        }
    }

    pub(super) fn ownership(&self, origin: Origin, error: OwnershipError) -> LifecycleTaskFailure {
        let (cause, detail) = match &error {
            OwnershipError::Stale => (CauseClass::OwnershipStale, DetailCode::NotApplicable),
            OwnershipError::IncompatibleProtocol => {
                (CauseClass::IncompatibleProtocol, DetailCode::NotApplicable)
            }
            OwnershipError::Invalid(_) => (CauseClass::OwnershipInvalid, DetailCode::Suppressed),
            OwnershipError::Unavailable(text) => (
                CauseClass::OwnershipUnavailable,
                match (origin, text.as_str()) {
                    // Exact source-owned constants from OwnershipCoordinator::bounded
                    // and Authority::revoke_transports. Never classify driver prose.
                    (
                        Origin::OwnershipAcquire | Origin::Handoff,
                        "ownership operation exceeded its deadline",
                    ) => DetailCode::OwnershipDeadline,
                    (Origin::Handoff, "receiving runtime revocation failed") => {
                        DetailCode::ReceiverRevocationUnconfirmed
                    }
                    _ => DetailCode::Suppressed,
                },
            ),
        };
        LifecycleTaskFailure {
            compatibility_error: SandboxStoreError::Conflict,
            observation: self.observe(origin, cause, detail),
        }
    }

    pub(super) fn terminated(&self, panic: bool) -> FailureObservation {
        self.observe(
            Origin::Unspecified,
            if panic {
                CauseClass::TaskPanicked
            } else {
                CauseClass::TaskCancelled
            },
            if panic {
                DetailCode::Suppressed
            } else {
                DetailCode::NotApplicable
            },
        )
    }
}

/// Only generated correlations, closed codes and bounded numeric fields can
/// inhabit the serialized view. Arbitrary underlying error data has no slot.
#[derive(Debug, Serialize)]
pub(super) struct FailureObservation {
    pub phase: Phase,
    pub origin: Origin,
    pub cause_class: CauseClass,
    pub detail_code: DetailCode,
    pub detail_retained: bool,
    pub detail_suppressed: bool,
    pub attempt_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_hash: Option<String>,
    pub elapsed_ms: u64,
    pub continuation_index: u8,
}

pub(super) struct LifecycleTaskFailure {
    compatibility_error: SandboxStoreError,
    pub observation: FailureObservation,
}

impl LifecycleTaskFailure {
    pub(super) fn into_compatibility(self) -> SandboxStoreError {
        self.compatibility_error
    }
}

// Result assertions may use Debug; it must be the safe view, never the source.
impl std::fmt::Debug for LifecycleTaskFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.observation.fmt(formatter)
    }
}

#[cfg(test)]
#[path = "lifecycle_failure_tests.rs"]
mod tests;
