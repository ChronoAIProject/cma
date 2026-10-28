use std::sync::Arc;

use super::store::{SandboxStore, SandboxStoreError};
use super::{
    Sandbox, SandboxStatus, SandboxUnpublishedRecoveryRequest, SandboxUnpublishedRecoveryVerifier,
};
use crate::{Caller, Capability, SandboxId, SubjectId};

#[derive(Debug, thiserror::Error)]
pub enum SandboxRuntimeUpdateError {
    #[error("forbidden")]
    Forbidden,
    #[error("sandbox not found")]
    NotFound,
    #[error("runtime update target changed")]
    TargetChanged,
    #[error("sandbox runtime update is not available")]
    NotAvailable,
    #[error("sandbox dependency unavailable")]
    Unavailable,
}

pub struct SandboxRuntimeUpdateService {
    store: Arc<dyn SandboxStore>,
    latest_version: String,
    recovery: Option<Arc<dyn SandboxUnpublishedRecoveryVerifier>>,
}

impl SandboxRuntimeUpdateService {
    pub fn new(store: Arc<dyn SandboxStore>, latest_version: String) -> Self {
        Self {
            store,
            latest_version,
            recovery: None,
        }
    }

    pub fn with_unpublished_recovery(
        mut self,
        verifier: Arc<dyn SandboxUnpublishedRecoveryVerifier>,
    ) -> Self {
        self.recovery = Some(verifier);
        self
    }

    pub fn latest_version(&self) -> &str {
        &self.latest_version
    }

    pub fn can_update(&self, sandbox: &Sandbox) -> bool {
        if self.recovery.is_some() && sandbox.unpublished_recovery_candidate() {
            return true;
        }
        if !sandbox.supports_retained_runtime_update() {
            return false;
        }
        if let Some(request) = sandbox
            .runtime_update_request()
            .filter(|r| r.completed_at.is_none())
        {
            return request.failure.is_some()
                && sandbox.work.lease.is_none()
                && sandbox.admitted_runtime_update_target()
                    == Some(request.target_version.as_str());
        }
        matches!(
            sandbox.status,
            SandboxStatus::Active | SandboxStatus::Paused
        ) && sandbox
            .retained_runtime_binding()
            .is_some_and(|binding| binding.runtime_version != self.latest_version)
    }

    pub async fn request(
        &self,
        caller: &Caller,
        id: &SandboxId,
        target_version: &str,
    ) -> Result<Sandbox, SandboxRuntimeUpdateError> {
        if !caller.authorization.allows(Capability::UseOwnSandboxes) {
            return Err(SandboxRuntimeUpdateError::Forbidden);
        }
        self.queue(id, target_version, Some(&caller.subject)).await
    }

    /// Queue the same retained-home update for an operator without impersonating its owner.
    pub async fn request_as_operator(
        &self,
        caller: &Caller,
        id: &SandboxId,
        target_version: &str,
    ) -> Result<Sandbox, SandboxRuntimeUpdateError> {
        if !caller.authorization.allows(Capability::ControlAnySandbox) {
            return Err(SandboxRuntimeUpdateError::Forbidden);
        }
        self.queue(id, target_version, None).await
    }

    /// Automatic repair is available once per unresolved recovery incident after
    /// ordinary retries park/fail. Its existing target must still be current;
    /// obsolete pinned targets require an explicit owner/operator update.
    pub async fn recover_failed_unpublished(
        &self,
        id: &SandboxId,
    ) -> Result<Sandbox, SandboxRuntimeUpdateError> {
        let sandbox = self
            .store
            .get_sandbox(id)
            .await
            .map_err(store_error)?
            .ok_or(SandboxRuntimeUpdateError::NotFound)?;
        let target = sandbox
            .admitted_runtime_update_target()
            .or_else(|| {
                sandbox
                    .retained_runtime_binding()
                    .map(|runtime| runtime.runtime_version.as_str())
            })
            .ok_or(SandboxRuntimeUpdateError::NotAvailable)?;
        if target != self.latest_version
            || sandbox.lifecycle.unpublished_recovery_attempted
            || sandbox.lifecycle.unpublished_recovery.is_some()
        {
            return Err(SandboxRuntimeUpdateError::NotAvailable);
        }
        self.recover_unpublished(&sandbox, target, true)
            .await
            .map_err(store_error)
    }

    async fn recover_unpublished(
        &self,
        sandbox: &Sandbox,
        target: &str,
        automatic: bool,
    ) -> Result<Sandbox, SandboxStoreError> {
        let verifier = self
            .recovery
            .as_ref()
            .ok_or(SandboxStoreError::InvalidTransaction(
                "unpublished runtime recovery is unavailable",
            ))?;
        let snapshot = self
            .store
            .unpublished_runtime_recovery_snapshot(&sandbox.id)
            .await?;
        if snapshot.sandbox != *sandbox {
            return Err(SandboxStoreError::Conflict);
        }
        snapshot.validate()?;
        let proof = verifier.verify(&snapshot).await?;
        self.store
            .admit_unpublished_runtime_recovery(SandboxUnpublishedRecoveryRequest {
                snapshot,
                proof,
                target_runtime_version: target.into(),
                automatic,
                requested_at: crate::now_rfc3339(),
            })
            .await
    }

    async fn queue(
        &self,
        id: &SandboxId,
        target_version: &str,
        owner: Option<&SubjectId>,
    ) -> Result<Sandbox, SandboxRuntimeUpdateError> {
        for _ in 0..8 {
            let mut sandbox = self
                .store
                .get_sandbox(id)
                .await
                .map_err(store_error)?
                .filter(|sandbox| owner.is_none_or(|owner| sandbox.owner == *owner))
                .ok_or(SandboxRuntimeUpdateError::NotFound)?;
            let pinned = sandbox.runtime_update_request().is_some_and(|request| {
                request.completed_at.is_none() && request.target_version == target_version
            });
            if target_version != self.latest_version && !pinned {
                return Err(SandboxRuntimeUpdateError::TargetChanged);
            }
            if sandbox.unpublished_recovery_candidate() {
                match self
                    .recover_unpublished(&sandbox, target_version, false)
                    .await
                {
                    Ok(recovered) => return Ok(recovered),
                    Err(SandboxStoreError::Conflict) => {
                        tokio::task::yield_now().await;
                        continue;
                    }
                    Err(error) => return Err(store_error(error)),
                }
            }
            let revision = sandbox.revision;
            if !sandbox
                .request_runtime_update(target_version.into(), false, crate::now_rfc3339())
                .map_err(|_| SandboxRuntimeUpdateError::NotAvailable)?
            {
                return Ok(sandbox);
            }
            match self.store.save_sandbox(revision, sandbox.clone()).await {
                Ok(()) => return Ok(sandbox),
                Err(SandboxStoreError::Conflict) => tokio::task::yield_now().await,
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(SandboxRuntimeUpdateError::Unavailable)
    }
}

fn store_error(error: SandboxStoreError) -> SandboxRuntimeUpdateError {
    match error {
        SandboxStoreError::NotFound => SandboxRuntimeUpdateError::NotFound,
        SandboxStoreError::InvalidTransaction(_) => SandboxRuntimeUpdateError::NotAvailable,
        _ => SandboxRuntimeUpdateError::Unavailable,
    }
}
