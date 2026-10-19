use std::sync::Arc;

use super::store::{SandboxStore, SandboxStoreError};
use super::{Sandbox, SandboxStatus};
use crate::{Caller, Capability, SandboxId};

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
}

impl SandboxRuntimeUpdateService {
    pub fn new(store: Arc<dyn SandboxStore>, latest_version: String) -> Self {
        Self {
            store,
            latest_version,
        }
    }

    pub fn latest_version(&self) -> &str {
        &self.latest_version
    }

    pub fn can_update(&self, sandbox: &Sandbox) -> bool {
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
        for _ in 0..8 {
            let mut sandbox = self
                .store
                .get_sandbox(id)
                .await
                .map_err(store_error)?
                .filter(|sandbox| sandbox.owner == caller.subject)
                .ok_or(SandboxRuntimeUpdateError::NotFound)?;
            let pinned = sandbox.runtime_update_request().is_some_and(|request| {
                request.completed_at.is_none() && request.target_version == target_version
            });
            if target_version != self.latest_version && !pinned {
                return Err(SandboxRuntimeUpdateError::TargetChanged);
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
        _ => SandboxRuntimeUpdateError::Unavailable,
    }
}
