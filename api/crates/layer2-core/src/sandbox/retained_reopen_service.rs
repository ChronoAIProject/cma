//! Explicit owner consent and idempotency before the proof-bound transaction.
use std::{sync::Arc, time::Duration};

use super::{
    store::{SandboxStore, SandboxStoreError},
    RetainedReopenAccepted, RetainedReopenRequest, RetainedReopenVerifier,
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
    #[error("this environment cannot safely reopen its conversations")]
    Refused,
    #[error("recovery is unavailable")]
    Unavailable,
}

pub struct RetainedReopenService {
    store: Arc<dyn SandboxStore>,
    verifier: Arc<dyn RetainedReopenVerifier>,
    enabled: bool,
    permits: tokio::sync::Semaphore,
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
            permits: tokio::sync::Semaphore::new(1),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
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
            if let Some(receipt) = self
                .store
                .retained_reopen_receipt(id)
                .await
                .map_err(map_error)?
            {
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
            if !self.enabled {
                return Err(E::Unavailable);
            }
            if parent.revision != expected_revision {
                return Err(E::Conflict);
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
            Ok(Ok(RetainedReopenRequest {
                snapshot,
                proof,
                key_sha256,
                requested_at: crate::now_rfc3339(),
            }))
        })
        .await
        .map_err(|_| E::Unavailable)??;
        let request = match prepared {
            Ok(request) => request,
            Err(accepted) => return Ok(accepted),
        };
        // Do not cancel a transaction with the read-only proof deadline. Its
        // store reconciles unknown commits and retains the exact accepted key.
        self.store
            .admit_retained_reopen(request)
            .await
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
