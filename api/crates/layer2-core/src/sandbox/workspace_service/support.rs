use super::*;
use crate::sandbox::SandboxOrigin;

pub(super) fn workspace(
    sandbox: &Sandbox,
    revision: &WorkspaceProfileRevision,
    origin: WorkspaceOrigin,
    now: String,
) -> Result<Workspace, WorkspaceServiceError> {
    revision
        .content
        .validate()
        .map_err(|error| WorkspaceServiceError::InvalidProfile(error.to_string()))?;
    let name = revision
        .content
        .workspace_name()
        .map_err(|error| WorkspaceServiceError::InvalidProfile(error.to_string()))?;
    let source = match &revision.content.location {
        WorkspaceLocation::RepositorySnapshot { url, reference, .. } => {
            WorkspaceSource::RepositorySnapshot {
                url: url.clone(),
                reference: reference.clone(),
                source_commit: None,
            }
        }
        WorkspaceLocation::PublicRepositorySnapshot {
            url,
            commit,
            operation_id,
        } => {
            if origin != (WorkspaceOrigin::SandboxProfile { index: 0 })
                || sandbox.origin
                    != (SandboxOrigin::Lifecycle {
                        operation_id: operation_id.clone(),
                    })
                || sandbox.profile.content.workspace_profiles.as_slice()
                    != [WorkspaceProfileRef {
                        id: revision.profile_id.clone(),
                        revision: revision.revision,
                    }]
            {
                return Err(WorkspaceServiceError::InvalidProfile(
                    "public repository source belongs to its original lifecycle allocation".into(),
                ));
            }
            WorkspaceSource::PublicRepositorySnapshot {
                url: url.clone(),
                commit: commit.clone(),
                source_commit: None,
            }
        }
        WorkspaceLocation::GitRepository { url, reference } => WorkspaceSource::GitRepository {
            url: url.clone(),
            reference: reference.clone(),
        },
        WorkspaceLocation::Directory { .. } => WorkspaceSource::Empty,
    };
    Ok(Workspace {
        id: WorkspaceId::generate(),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        origin,
        profile: WorkspaceProfileSnapshot::from(revision),
        name: name.clone(),
        path: format!("/home/cma/{name}"),
        source,
        resolved_commit: None,
        status: WorkspaceStatus::Preparing,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, now.clone()),
        next_worktree_number: 1,
        created_at: now,
        ready_at: None,
        failed: None,
        removed_at: None,
        revision: 1,
    })
}

pub(super) fn require(caller: &Caller) -> Result<(), WorkspaceServiceError> {
    caller
        .authorization
        .allows(Capability::UseOwnSandboxes)
        .then_some(())
        .ok_or(WorkspaceServiceError::Forbidden)
}

pub(super) fn require_launch(
    caller: &Caller,
    authority: Capability,
) -> Result<(), WorkspaceServiceError> {
    (matches!(
        authority,
        Capability::LaunchTriggers | Capability::CreateSandbox
    ) && caller.authorization.allows(authority))
    .then_some(())
    .ok_or(WorkspaceServiceError::Forbidden)
}

pub(super) fn trust_error(error: MarketplaceError) -> WorkspaceServiceError {
    match error {
        MarketplaceError::TrustConfirmationRequired { commands_digest } => {
            WorkspaceServiceError::TrustConfirmationRequired {
                digest: commands_digest,
            }
        }
        other => WorkspaceServiceError::Unavailable(other.to_string()),
    }
}

pub(super) fn profile_error(error: ProfileStoreError) -> WorkspaceServiceError {
    match error {
        ProfileStoreError::NotFound => WorkspaceServiceError::ProfileNotFound,
        ProfileStoreError::Corrupt(detail) => WorkspaceServiceError::Corrupt(detail),
        other => WorkspaceServiceError::Unavailable(other.to_string()),
    }
}

pub(super) fn store_error(error: SandboxStoreError) -> WorkspaceServiceError {
    match error {
        SandboxStoreError::NotFound => WorkspaceServiceError::NotFound,
        SandboxStoreError::SandboxNotReady => WorkspaceServiceError::SandboxNotReady,
        SandboxStoreError::WorkspaceNameConflict | SandboxStoreError::Duplicate => {
            WorkspaceServiceError::NameConflict
        }
        SandboxStoreError::WorkspaceLimitReached { limit, live } => {
            WorkspaceServiceError::LimitReached { limit, live }
        }
        SandboxStoreError::WorkspaceHasLiveAgents => WorkspaceServiceError::HasLiveAgents,
        SandboxStoreError::WorkspaceOriginConflict => WorkspaceServiceError::OriginConflict,
        SandboxStoreError::WorkspaceRecoveryConflict => WorkspaceServiceError::RecoveryConflict,
        SandboxStoreError::Budget(reason) => WorkspaceServiceError::Budget(reason),
        SandboxStoreError::AdmissionPaused => WorkspaceServiceError::AdmissionPaused,
        SandboxStoreError::Corrupt(detail) => WorkspaceServiceError::Corrupt(detail),
        other => WorkspaceServiceError::Unavailable(other.to_string()),
    }
}

impl WorkspaceService {
    pub async fn recover_prepare(
        &self,
        caller: &Caller,
        id: &WorkspaceId,
        operation_id: &str,
    ) -> Result<Workspace, WorkspaceServiceError> {
        if !caller.authorization.allows(Capability::ControlAnySandbox) {
            require(caller)?;
        }
        self.store
            .begin_workspace_prepare_recovery(id, caller, operation_id, now_rfc3339())
            .await
            .map_err(store_error)
    }
}
