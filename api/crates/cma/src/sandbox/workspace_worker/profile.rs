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
        let workspace = self
            .worker
            .run_until_ready(&workspace.id, SystemTime::now())
            .await
            .map_err(|error| SandboxDependencyError::Transient(error.to_string()))?;
        match workspace.status {
            WorkspaceStatus::Ready => Ok(()),
            WorkspaceStatus::Failed => Err(SandboxDependencyError::Validation(
                workspace.failed.map_or_else(
                    || "workspace preparation failed".into(),
                    |failure| failure.detail,
                ),
            )),
            _ => Err(SandboxDependencyError::Transient(
                "workspace lifecycle has not reached ready".into(),
            )),
        }
    }
}
