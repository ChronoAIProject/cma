use super::*;

impl OperatorError {
    pub(super) fn from_sandbox_store(error: SandboxStoreError) -> Self {
        match error {
            SandboxStoreError::Budget(_)
            | SandboxStoreError::AdmissionPaused
            | SandboxStoreError::SessionCapacityUnavailable => Self::Unavailable(error.to_string()),
            SandboxStoreError::GlobalCapacityReached => Self::Unavailable(error.to_string()),
            SandboxStoreError::NotFound => Self::NotFound,
            SandboxStoreError::InvalidPage => Self::InvalidRequest(error.to_string()),
            SandboxStoreError::AccountDeletionInProgress
            | SandboxStoreError::Conflict
            | SandboxStoreError::CheckpointInProgress
            | SandboxStoreError::LifecycleBusy { .. }
            | SandboxStoreError::Duplicate
            | SandboxStoreError::CaptureAdvanced
            | SandboxStoreError::SequenceConflict
            | SandboxStoreError::LimitReached { .. }
            | SandboxStoreError::SandboxNotReady
            | SandboxStoreError::WorkspaceNameConflict
            | SandboxStoreError::WorkspaceLimitReached { .. }
            | SandboxStoreError::WorkspaceHasLiveAgents
            | SandboxStoreError::WorkspaceRecoveryConflict
            | SandboxStoreError::WorkspaceOriginConflict
            | SandboxStoreError::SandboxOriginConflict
            | SandboxStoreError::WorkspaceNotReady
            | SandboxStoreError::AgentLimitReached { .. }
            | SandboxStoreError::AgentCreationConflict
            | SandboxStoreError::AgentLogCapacityExceeded
            | SandboxStoreError::InvalidTransaction(_) => Self::InvalidState(error.to_string()),
            SandboxStoreError::Unavailable(_) | SandboxStoreError::Corrupt(_) => {
                Self::Unavailable(error.to_string())
            }
        }
    }

    pub(super) fn from_profile_store(error: ProfileStoreError) -> Self {
        match error {
            ProfileStoreError::NotFound => Self::NotFound,
            ProfileStoreError::Conflict
            | ProfileStoreError::Duplicate
            | ProfileStoreError::ReferenceUnavailable => Self::InvalidState(error.to_string()),
            ProfileStoreError::Protected => Self::ProtectedProfile,
            ProfileStoreError::Validation(_) => Self::InvalidRequest(error.to_string()),
            ProfileStoreError::Unavailable(_) | ProfileStoreError::Corrupt(_) => {
                Self::Unavailable(error.to_string())
            }
        }
    }
}
