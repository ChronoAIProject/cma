use super::*;
use crate::sandbox::worker::{ProfileWorkspaceProvisioner, SandboxDependencyError};
use layer2_core::profiles::WorkspaceProfileRef;
use layer2_core::sandbox::WorkspaceService;

pub(crate) struct DurableProfileWorkspaceProvisioner {
    service: Arc<WorkspaceService>,
    worker: Arc<WorkspaceWorker>,
}

impl DurableProfileWorkspaceProvisioner {
    pub fn new(service: Arc<WorkspaceService>, worker: Arc<WorkspaceWorker>) -> Self {
        Self { service, worker }
    }
}

#[async_trait::async_trait]
impl ProfileWorkspaceProvisioner for DurableProfileWorkspaceProvisioner {
    fn requires_runtime_adoption(&self) -> bool {
        false
    }

    async fn create_profile_workspace(
        &self,
        sandbox: &Sandbox,
        _runtime: &RuntimeHandle,
        profile: &WorkspaceProfileRef,
        index: u16,
        _operation_id: &str,
    ) -> Result<(), SandboxDependencyError> {
        let workspace = self
            .service
            .create_profile_workspace(sandbox, profile, index)
            .await
            .map_err(|error| match error {
                layer2_core::sandbox::WorkspaceServiceError::Unavailable(detail) => {
                    SandboxDependencyError::Transient(detail)
                }
                error @ layer2_core::sandbox::WorkspaceServiceError::LimitReached { .. } => {
                    SandboxDependencyError::ResourceQuota(error.to_string())
                }
                error => SandboxDependencyError::Validation(error.to_string()),
            })?;
        let drained = self
            .worker
            .run_until_ready_observing_bootstrap(&workspace.id, sandbox, SystemTime::now())
            .await
            .map_err(|error| SandboxDependencyError::Transient(error.to_string()))?;
        let workspace = drained.workspace;
        if !drained.executed_work {
            // A retry can find a completed or currently unclaimable child.
            // Retain the runtime proof previously supplied by the outer worker.
            self.worker.runtime(&workspace).await.map_err(|failure| {
                if failure.retryable {
                    SandboxDependencyError::Transient(failure.detail.into())
                } else {
                    runtime_failure(failure.class, failure.detail.into())
                }
            })?;
        }
        match workspace.status {
            WorkspaceStatus::Ready => Ok(()),
            WorkspaceStatus::Failed => Err(workspace.failed.map_or_else(
                || SandboxDependencyError::Validation("workspace preparation failed".into()),
                |failure| runtime_failure(&failure.class, failure.detail),
            )),
            _ => Err(SandboxDependencyError::Transient(
                "workspace lifecycle has not reached ready".into(),
            )),
        }
    }
}

fn runtime_failure(class: &str, detail: String) -> SandboxDependencyError {
    match class {
        "workspace_resource_quota" => SandboxDependencyError::ResourceQuota(detail),
        "workspace_runtime_contract_mismatch" => SandboxDependencyError::Security(detail),
        _ => SandboxDependencyError::Validation(detail),
    }
}
