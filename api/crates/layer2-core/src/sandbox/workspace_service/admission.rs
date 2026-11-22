//! Select catalogue pin admission while preserving inline snapshots.
use super::*;

impl WorkspaceService {
    pub(super) async fn admit(
        &self,
        sandbox: &Sandbox,
        revision: &WorkspaceProfileRevision,
        origin: WorkspaceOrigin,
        kind: WorkspaceAdmissionKind,
        catalogue: bool,
    ) -> Result<Workspace, WorkspaceServiceError> {
        let now = now_rfc3339();
        let workspace = workspace(sandbox, revision, origin, now.clone())?;
        let admission = WorkspaceAdmission {
            binding: None,
            workspace,
            kind,
            event_at: now,
        };
        let result = if catalogue {
            self.store.admit_catalogue_workspace(admission).await
        } else {
            self.store.admit_workspace(admission).await
        }
        .map_err(store_error)?;
        match result {
            WorkspaceAdmissionResult::Existing(workspace) => Ok(workspace),
            WorkspaceAdmissionResult::Created(workspace) => {
                let locator = MarketplaceLocator::Workspace(workspace.profile.id.clone());
                if let Err(error) = self.profile_uses.record_profile_use(&locator).await {
                    tracing::warn!(
                        workspace_id = %workspace.id,
                        profile_id = locator.as_str(),
                        %error,
                        "workspace profile use was not recorded"
                    );
                }
                Ok(workspace)
            }
        }
    }
}
