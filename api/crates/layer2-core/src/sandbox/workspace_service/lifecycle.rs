//! Durable empty workspace admission for an existing Sandbox.
use super::*;
use crate::resource_access::ResourceActor;

impl WorkspaceService {
    pub async fn authorize_lifecycle_sandbox(
        &self,
        caller: &Caller,
        actor: Option<&ResourceActor>,
        id: &SandboxId,
    ) -> Result<Sandbox, WorkspaceServiceError> {
        require(caller)?;
        if actor.is_some_and(|actor| actor.ordinary_subject() != Some(&caller.subject)) {
            return Err(WorkspaceServiceError::Forbidden);
        }
        let sandbox = self.owned_sandbox(caller, id).await?;
        if !sandbox.accepts_creation_admission() {
            return Err(WorkspaceServiceError::SandboxNotReady);
        }
        self.connections.check_sandbox(&sandbox).await?;
        Ok(sandbox)
    }

    /// The operation origin and profile identity survive worker replacement.
    /// Transactional admission returns the same Workspace under concurrent retries.
    pub async fn create_for_lifecycle(
        &self,
        caller: &Caller,
        actor: Option<&ResourceActor>,
        sandbox_id: &SandboxId,
        profile: &WorkspaceProfileRef,
        operation_id: &str,
    ) -> Result<Workspace, WorkspaceServiceError> {
        if operation_id.is_empty()
            || operation_id.len() > 128
            || !operation_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(WorkspaceServiceError::InvalidOrigin);
        }
        let sandbox = self
            .authorize_lifecycle_sandbox(caller, actor, sandbox_id)
            .await?;
        let (owner, revision) = self.read_visible_revision(&caller.subject, profile).await?;
        // This entry point only creates the owner's empty, instruction-free workspace.
        if owner != caller.subject
            || !matches!(
                revision.content.location,
                WorkspaceLocation::Directory { .. }
            )
            || !revision.content.setup_commands.is_empty()
            || !revision.content.agent_profiles.is_empty()
        {
            return Err(WorkspaceServiceError::InvalidProfile(
                "lifecycle requires an owned empty workspace".into(),
            ));
        }
        self.admit(
            &sandbox,
            &revision,
            WorkspaceOrigin::Lifecycle {
                operation_id: operation_id.into(),
            },
            WorkspaceAdmissionKind::Lifecycle,
        )
        .await
    }
}
