//! Owner-facing commands for reversible sandbox lifecycle changes.

use crate::{now_rfc3339, Caller, Capability, SandboxId};

use super::store::{
    SandboxLifecycleCommand, SandboxLifecycleCommandKind, SandboxLifecycleCommandResult,
    SandboxStoreError,
};
use super::{SandboxLifecycleCommandOutcome, SandboxReviveCause, SandboxService, SandboxStatus};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandboxLifecycleCommandError {
    #[error("forbidden")]
    Forbidden,
    #[error("sandbox not found")]
    NotFound,
    #[error("sandbox is not active (current status: {status:?})")]
    SandboxNotActive { status: SandboxStatus },
    #[error("sandbox is not paused (current status: {status:?})")]
    SandboxNotPaused { status: SandboxStatus },
    #[error("sandbox data is not usable: {0}")]
    Corrupt(String),
    #[error("sandbox lifecycle dependency is unavailable: {0}")]
    Unavailable(String),
}

impl SandboxLifecycleCommandError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Forbidden => "forbidden",
            Self::NotFound => "sandbox_not_found",
            Self::SandboxNotActive { .. } => "sandbox_not_active",
            Self::SandboxNotPaused { .. } => "sandbox_not_paused",
            Self::Corrupt(_) => "stored_sandbox_corrupt",
            Self::Unavailable(_) => "sandbox_lifecycle_unavailable",
        }
    }
}

impl SandboxService {
    pub async fn pause(
        &self,
        caller: &Caller,
        id: &SandboxId,
    ) -> Result<SandboxLifecycleCommandOutcome, SandboxLifecycleCommandError> {
        require_owner_authority(caller)?;
        let command =
            SandboxLifecycleCommand::pause(id.clone(), caller.subject.clone(), now_rfc3339(), None);
        self.schedule_owner_lifecycle(command).await
    }

    pub async fn revive(
        &self,
        caller: &Caller,
        id: &SandboxId,
    ) -> Result<SandboxLifecycleCommandOutcome, SandboxLifecycleCommandError> {
        require_owner_authority(caller)?;
        let command = SandboxLifecycleCommand::revive(
            id.clone(),
            caller.subject.clone(),
            now_rfc3339(),
            SandboxReviveCause::ExplicitRequest,
        );
        self.schedule_owner_lifecycle(command).await
    }

    async fn schedule_owner_lifecycle(
        &self,
        command: SandboxLifecycleCommand,
    ) -> Result<SandboxLifecycleCommandOutcome, SandboxLifecycleCommandError> {
        let kind = command.kind.clone();
        match self
            .store
            .schedule_sandbox_lifecycle(command)
            .await
            .map_err(lifecycle_store_error)?
        {
            SandboxLifecycleCommandResult::Accepted(outcome) => Ok(outcome),
            SandboxLifecycleCommandResult::NotEligible(sandbox) => match kind {
                SandboxLifecycleCommandKind::Pause { .. } => {
                    Err(SandboxLifecycleCommandError::SandboxNotActive {
                        status: sandbox.status,
                    })
                }
                SandboxLifecycleCommandKind::Revive { .. } => {
                    Err(SandboxLifecycleCommandError::SandboxNotPaused {
                        status: sandbox.status,
                    })
                }
            },
        }
    }
}

fn require_owner_authority(caller: &Caller) -> Result<(), SandboxLifecycleCommandError> {
    caller
        .authorization
        .allows(Capability::UseOwnSandboxes)
        .then_some(())
        .ok_or(SandboxLifecycleCommandError::Forbidden)
}

fn lifecycle_store_error(error: SandboxStoreError) -> SandboxLifecycleCommandError {
    match error {
        SandboxStoreError::NotFound => SandboxLifecycleCommandError::NotFound,
        SandboxStoreError::Corrupt(detail) => SandboxLifecycleCommandError::Corrupt(detail),
        other => SandboxLifecycleCommandError::Unavailable(other.to_string()),
    }
}
