//! Explicit owner consent and idempotency before the proof-bound transaction.
use std::{sync::Arc, time::Duration};

use super::{
    retained_recovery::{store_error, RetainedRecoverySource},
    store::{SandboxStore, SandboxStoreError},
    AgentStatus, RetainedRecoveryAssessment, RetainedRecoveryDisposition as D,
    RetainedRecoveryError, RetainedRecoverySnapshot, RetainedReopenAccepted, RetainedReopenRequest,
    RetainedReopenVerifier, RetainedSuspendedReopenRequest,
};
use crate::{Caller, Capability, SandboxId};

#[derive(Debug, thiserror::Error)]
pub enum RetainedReopenError {
    #[error("forbidden")]
    Forbidden,
    #[error("sandbox not found")]
    NotFound,
    #[error("explicit same-image recovery consent is required")]
    InvalidConsent,
    #[error("recovery authority changed; review the current Sandbox")]
    Conflict,
    #[error("the requested revision passed without accepting this recovery key")]
    StaleRequest,
    #[error("this environment cannot safely reopen its conversations")]
    Refused,
    #[error("recovery is unavailable")]
    Unavailable,
}

pub struct RetainedReopenService {
    store: Arc<dyn SandboxStore>,
    verifier: Arc<dyn RetainedReopenVerifier>,
    enabled: bool,
    suspended_enabled: bool,
    repeat_enabled: bool,
    permits: tokio::sync::Semaphore,
}

enum PreparedAdmission {
    Idle(RetainedReopenRequest),
    Suspended(RetainedSuspendedReopenRequest),
}

#[derive(Clone, Copy)]
pub enum RetainedReopenPath {
    Idle,
    Suspended,
}

impl RetainedReopenService {
    pub fn new(
        store: Arc<dyn SandboxStore>,
        verifier: Arc<dyn RetainedReopenVerifier>,
        enabled: bool,
    ) -> Self {
        Self {
            store,
            verifier,
            enabled,
            suspended_enabled: false,
            repeat_enabled: false,
            permits: tokio::sync::Semaphore::new(1),
        }
    }

    pub fn with_suspended_admission(mut self, enabled: bool) -> Self {
        self.suspended_enabled = enabled;
        self
    }

    pub fn with_repeat_admission(mut self, enabled: bool) -> Self {
        self.repeat_enabled = enabled;
        self
    }

    fn enabled(&self) -> bool {
        self.enabled || self.suspended_enabled
    }

    fn permits_revision(&self, parent: &super::Sandbox, latest_revision: Option<u64>) -> bool {
        latest_revision.is_none_or(|latest| {
            self.repeat_enabled
                && parent.status == super::SandboxStatus::Failed
                && parent.revision > latest
        })
    }

    pub(super) async fn assess_admission(
        &self,
        initial: &RetainedRecoverySnapshot,
        source: &Result<RetainedRecoverySource<'_>, D>,
    ) -> Result<Option<RetainedRecoveryAssessment>, RetainedRecoveryError> {
        let path = match source {
            Ok(_) if self.enabled => RetainedReopenPath::Idle,
            Err(D::ChildActivity) if self.suspended_enabled => RetainedReopenPath::Suspended,
            _ => return Ok(None),
        };
        let id = &initial.sandbox.id;
        let snapshot = match self
            .store
            .retained_reopen_assessment_snapshot(id, path)
            .await
        {
            Ok(snapshot) => snapshot,
            Err(SandboxStoreError::InvalidTransaction(_) | SandboxStoreError::Conflict) => {
                return Ok(None);
            }
            Err(error) => return Err(store_error(error)),
        };
        if snapshot.retained.sandbox.owner != initial.sandbox.owner {
            return Err(RetainedRecoveryError::NotFound);
        }
        if &snapshot.retained != initial {
            return Ok(Some(RetainedRecoveryAssessment::inspection(
                D::AuthorityChanged,
            )));
        }
        let disposition = match path {
            RetainedReopenPath::Idle if snapshot.validate().is_ok() => {
                let Ok(proof) = self.verifier.verify(&snapshot).await else {
                    return Ok(None);
                };
                if proof.validate(&snapshot).is_err() {
                    return Ok(None);
                }
                if source
                    .as_ref()
                    .is_ok_and(|source| source.expected_boot == proof.observed_boot)
                {
                    D::RecordedBootPresent
                } else {
                    D::ExactBootReplacement
                }
            }
            RetainedReopenPath::Suspended if snapshot.suspended_source().is_ok() => {
                match self.verifier.verify_suspended(&snapshot).await {
                    Ok(proof) if proof.validate(&snapshot).is_ok() => D::ExactSuspendedRuntime,
                    Err(SandboxStoreError::Unavailable(_)) => {
                        return Ok(Some(RetainedRecoveryAssessment::inspection(
                            D::EvidenceUnavailable,
                        )));
                    }
                    _ => {
                        return Ok(Some(RetainedRecoveryAssessment::inspection(
                            D::ProviderEvidenceRefused,
                        )));
                    }
                }
            }
            _ => return Ok(None),
        };
        let current = self
            .store
            .retained_reopen_assessment_snapshot(id, path)
            .await
            .map_err(store_error)?;
        if current.retained.sandbox.owner != initial.sandbox.owner {
            return Err(RetainedRecoveryError::NotFound);
        }
        if current != snapshot {
            return Ok(Some(RetainedRecoveryAssessment::inspection(
                D::AuthorityChanged,
            )));
        }
        let history = self
            .store
            .retained_reopen_history(id, None)
            .await
            .map_err(store_error)?;
        Ok(Some(RetainedRecoveryAssessment {
            disposition,
            admission_enabled: self
                .permits_revision(&current.retained.sandbox, history.latest_revision),
        }))
    }

    pub async fn request(
        &self,
        caller: &Caller,
        id: &SandboxId,
        expected_revision: u64,
        key: &str,
        consent: bool,
    ) -> Result<RetainedReopenAccepted, RetainedReopenError> {
        let started = tokio::time::Instant::now();
        let result = self
            .request_checked(caller, id, expected_revision, key, consent)
            .await;
        let outcome = match &result {
            Ok(_) => "accepted_or_replayed",
            Err(RetainedReopenError::Forbidden) => "forbidden",
            Err(RetainedReopenError::NotFound) => "not_found",
            Err(RetainedReopenError::InvalidConsent) => "invalid_consent",
            Err(RetainedReopenError::Conflict) => "authority_changed",
            Err(RetainedReopenError::StaleRequest) => "stale_request",
            Err(RetainedReopenError::Refused) => "proof_refused",
            Err(RetainedReopenError::Unavailable) => "unavailable",
        };
        tracing::info!(parent: None, stage = "owner_admission", outcome,
            elapsed_ms = started.elapsed().as_millis() as u64, "retained conversation reopening");
        result
    }

    async fn request_checked(
        &self,
        caller: &Caller,
        id: &SandboxId,
        expected_revision: u64,
        key: &str,
        consent: bool,
    ) -> Result<RetainedReopenAccepted, RetainedReopenError> {
        use RetainedReopenError as E;
        if !caller.authorization.allows(Capability::UseOwnSandboxes) {
            return Err(E::Forbidden);
        }
        if !consent
            || expected_revision == 0
            || key.is_empty()
            || key.len() > 128
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_:".contains(&b))
        {
            return Err(E::InvalidConsent);
        }
        let _permit = self.permits.try_acquire().map_err(|_| E::Unavailable)?;
        let key_sha256 = super::retained_reopen::digest(&key).map_err(map_error)?;
        let prepared = tokio::time::timeout(Duration::from_secs(25), async {
            // Owner access precedes all child, receipt and native evidence.
            let parent = self
                .store
                .get_sandbox(id)
                .await
                .map_err(map_error)?
                .filter(|parent| parent.owner == caller.subject)
                .ok_or(E::NotFound)?;
            let history = self
                .store
                .retained_reopen_history(id, Some(&key_sha256))
                .await
                .map_err(map_error)?;
            if history
                .observed_parent
                .as_ref()
                .is_some_and(|(owner, _)| owner != &caller.subject)
            {
                return Err(E::NotFound);
            }
            if let Some(receipt) = history.replay {
                if receipt.owner != caller.subject {
                    return Err(E::NotFound);
                }
                if receipt.expected_revision != expected_revision
                    || receipt.key_sha256 != key_sha256
                {
                    return Err(E::Conflict);
                }
                // Disabling new admission does not revoke an accepted request.
                return Ok(Err(receipt.accepted));
            }
            if history
                .observed_parent
                .is_some_and(|(_, revision)| revision > expected_revision)
            {
                return Err(E::StaleRequest);
            }
            if !self.permits_revision(&parent, history.latest_revision) {
                return Err(E::Conflict);
            }
            let repeat_authorized = history.latest_revision.is_some();
            if !self.enabled() {
                return Err(E::Unavailable);
            }
            if parent.revision != expected_revision {
                return Err(E::Conflict);
            }
            if self.suspended_enabled && parent.status == super::SandboxStatus::Failed {
                let snapshot = self
                    .store
                    .retained_suspended_reopen_snapshot(id)
                    .await
                    .map_err(map_error)?;
                if snapshot.retained.sandbox != parent {
                    return Err(E::Conflict);
                }
                if snapshot.retained.agents.iter().any(|agent| {
                    matches!(
                        agent.status,
                        AgentStatus::Ready | AgentStatus::Working | AgentStatus::WaitingApproval
                    )
                }) {
                    snapshot.suspended_source().map_err(map_error)?;
                    let proof = self
                        .verifier
                        .verify_suspended(&snapshot)
                        .await
                        .map_err(map_error)?;
                    proof.validate(&snapshot).map_err(map_error)?;
                    return Ok(Ok(PreparedAdmission::Suspended(
                        RetainedSuspendedReopenRequest {
                            snapshot,
                            proof,
                            key_sha256,
                            requested_at: crate::now_rfc3339(),
                            repeat_authorized,
                        },
                    )));
                }
            }
            if !self.enabled {
                return Err(E::Unavailable);
            }
            let snapshot = self
                .store
                .retained_reopen_snapshot(id)
                .await
                .map_err(map_error)?;
            if snapshot.retained.sandbox != parent {
                return Err(E::Conflict);
            }
            snapshot.validate().map_err(map_error)?;
            let proof = self.verifier.verify(&snapshot).await.map_err(map_error)?;
            proof.validate(&snapshot).map_err(map_error)?;
            Ok(Ok(PreparedAdmission::Idle(RetainedReopenRequest {
                snapshot,
                proof,
                key_sha256,
                requested_at: crate::now_rfc3339(),
                repeat_authorized,
            })))
        })
        .await
        .map_err(|_| E::Unavailable)??;
        // Do not cancel a transaction with the read-only proof deadline. Its
        // store reconciles unknown commits and retains the exact accepted key.
        match prepared {
            Ok(PreparedAdmission::Idle(request)) => self.store.admit_retained_reopen(request).await,
            Ok(PreparedAdmission::Suspended(request)) => {
                self.store.admit_suspended_reopen(request).await
            }
            Err(accepted) => return Ok(accepted),
        }
        .map_err(map_error)
    }
}

fn map_error(error: SandboxStoreError) -> RetainedReopenError {
    match error {
        SandboxStoreError::NotFound => RetainedReopenError::NotFound,
        SandboxStoreError::Conflict | SandboxStoreError::Duplicate => RetainedReopenError::Conflict,
        SandboxStoreError::Unavailable(_) => RetainedReopenError::Unavailable,
        _ => RetainedReopenError::Refused,
    }
}
