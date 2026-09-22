use super::*;
use crate::sandbox::binding::{BindingAdmissionFence, ProviderBinding};

impl WorkspaceService {
    pub(crate) async fn create_for_binding(
        &self,
        binding: &ProviderBinding,
        revision: &WorkspaceProfileRevision,
    ) -> Result<Workspace, WorkspaceServiceError> {
        let sandbox = self
            .store
            .get_sandbox(&binding.sandbox_id)
            .await
            .map_err(store_error)?
            .filter(|value| value.owner == binding.owner)
            .ok_or(WorkspaceServiceError::NotFound)?;
        let at = now_rfc3339();
        let mut candidate = workspace(&sandbox, revision, WorkspaceOrigin::Manual, at.clone())?;
        candidate.id = binding.workspace_id.clone();
        let fence = BindingAdmissionFence::environment(binding);
        fence
            .require_workspace(binding, &candidate)
            .map_err(store_error)?;
        if let Some(existing) = self
            .store
            .get_workspace(&candidate.id)
            .await
            .map_err(store_error)?
        {
            fence
                .require_workspace(binding, &existing)
                .map_err(store_error)?;
            if existing.profile != candidate.profile {
                return Err(WorkspaceServiceError::OriginConflict);
            }
            return Ok(existing);
        }
        self.connections.check_sandbox(&sandbox).await?;
        let result = self
            .store
            .admit_workspace(WorkspaceAdmission {
                binding: Some(fence),
                workspace: candidate,
                kind: WorkspaceAdmissionKind::Manual,
                event_at: at,
            })
            .await
            .map_err(store_error)?;
        match result {
            WorkspaceAdmissionResult::Created(value)
            | WorkspaceAdmissionResult::Existing(value) => Ok(value),
        }
    }
}
