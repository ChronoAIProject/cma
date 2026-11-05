//! Owner-facing commands for reversible sandbox lifecycle changes.

use crate::{now_rfc3339, Caller, Capability, SandboxId};

use super::store::{
    SandboxLifecycleCommand, SandboxLifecycleCommandKind, SandboxLifecycleCommandResult,
    SandboxStoreError,
};
use super::{
    SandboxLifecycleBlocker, SandboxLifecycleCommandOutcome, SandboxReviveCause, SandboxService,
    SandboxStatus,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandboxLifecycleCommandError {
    #[error("forbidden")]
    Forbidden,
    #[error("sandbox not found")]
    NotFound,
    #[error("renewable sandbox credential setup is required")]
    CredentialSetupRequired,
    #[error("required connections have not been verified")]
    ConnectionsRequired { services: Vec<crate::ServiceId> },
    #[error("sandbox is not active (current status: {status:?})")]
    SandboxNotActive { status: SandboxStatus },
    #[error("sandbox is not paused (current status: {status:?})")]
    SandboxNotPaused { status: SandboxStatus },
    #[error("sandbox lifecycle is busy (current status: {status:?}, reason: {reason:?})")]
    SandboxBusy {
        status: SandboxStatus,
        reason: SandboxLifecycleBlocker,
    },
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
            Self::CredentialSetupRequired => "sandbox_credential_setup_required",
            Self::ConnectionsRequired { .. } => "connections_required",
            Self::SandboxNotActive { .. } => "sandbox_not_active",
            Self::SandboxNotPaused { .. } => "sandbox_not_paused",
            Self::SandboxBusy { .. } => "sandbox_lifecycle_busy",
            Self::Corrupt(_) => "stored_sandbox_corrupt",
            Self::Unavailable(_) => "sandbox_lifecycle_unavailable",
        }
    }
}

impl SandboxService {
    /// Internal command preparation keeps connection custody with the actual
    /// resource owner. The proof supplies independent current Sandbox authority.
    pub async fn revive_for_command(
        &self,
        authorization: std::sync::Arc<super::agent_io::CommandRevivalAuthorization>,
    ) -> Result<SandboxLifecycleCommandResult, SandboxLifecycleCommandError> {
        use super::agent_io::{command_access_error_kind, observe_command_preparation};
        let command = authorization.command();
        let snapshot = observe_command_preparation(
            command,
            "revival_snapshot",
            authorization.current_snapshot(),
            command_access_error_kind,
        )
        .await
        .map_err(|_| SandboxLifecycleCommandError::Forbidden)?;
        let sandbox = snapshot.lineage.sandbox;
        observe_command_preparation(
            command,
            "revival_prerequisites",
            self.credentials.check_services(
                &sandbox.owner,
                &sandbox
                    .profile
                    .content
                    .connection_services_for_grants(&sandbox.credential.effective_reaches()),
            ),
            |error| match error {
                super::SandboxCredentialPrerequisiteError::SetupRequired => {
                    "credential_setup_required"
                }
                super::SandboxCredentialPrerequisiteError::ConnectionsRequired { .. } => {
                    "connections_required"
                }
                super::SandboxCredentialPrerequisiteError::Unavailable(_) => {
                    "prerequisite_unavailable"
                }
            },
        )
        .await
        .map_err(|error| match error {
            super::SandboxCredentialPrerequisiteError::SetupRequired => {
                SandboxLifecycleCommandError::CredentialSetupRequired
            }
            super::SandboxCredentialPrerequisiteError::ConnectionsRequired { services } => {
                SandboxLifecycleCommandError::ConnectionsRequired { services }
            }
            super::SandboxCredentialPrerequisiteError::Unavailable(_) => {
                SandboxLifecycleCommandError::Unavailable(
                    "connection verification is unavailable".into(),
                )
            }
        })?;
        observe_command_preparation(
            command,
            "revival_scheduling",
            async {
                self.store
                    .schedule_command_revival(authorization.clone())
                    .await
                    .map_err(lifecycle_store_error)
            },
            SandboxLifecycleCommandError::code,
        )
        .await
    }

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
        self.require_connections(caller, id)
            .await
            .map_err(|error| match error {
                super::SandboxServiceError::NotFound => SandboxLifecycleCommandError::NotFound,
                super::SandboxServiceError::ConnectionsRequired { services } => {
                    SandboxLifecycleCommandError::ConnectionsRequired { services }
                }
                super::SandboxServiceError::CredentialSetupRequired => {
                    SandboxLifecycleCommandError::CredentialSetupRequired
                }
                _ => SandboxLifecycleCommandError::Unavailable(
                    "connection verification is unavailable".into(),
                ),
            })?;
        let command = SandboxLifecycleCommand::revive(
            id.clone(),
            caller.subject.clone(),
            now_rfc3339(),
            SandboxReviveCause::ExplicitRequest,
        );
        self.schedule_owner_lifecycle(command).await
    }

    pub async fn require_connections(
        &self,
        caller: &Caller,
        id: &SandboxId,
    ) -> Result<(), super::SandboxServiceError> {
        let sandbox = self
            .store
            .get_sandbox(id)
            .await
            .map_err(super::service::store_error)?
            .filter(|sandbox| {
                sandbox.owner == caller.subject && sandbox.status != SandboxStatus::Retired
            })
            .ok_or(super::SandboxServiceError::NotFound)?;
        self.credentials
            .check_services(
                &caller.subject,
                &sandbox
                    .profile
                    .content
                    .connection_services_for_grants(&sandbox.credential.effective_reaches()),
            )
            .await
            .map_err(|error| match error {
                super::SandboxCredentialPrerequisiteError::SetupRequired => {
                    super::SandboxServiceError::CredentialSetupRequired
                }
                super::SandboxCredentialPrerequisiteError::ConnectionsRequired { services } => {
                    super::SandboxServiceError::ConnectionsRequired { services }
                }
                super::SandboxCredentialPrerequisiteError::Unavailable(detail) => {
                    super::SandboxServiceError::Unavailable(detail)
                }
            })
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
                SandboxLifecycleCommandKind::Pause { .. }
                    if sandbox.status == SandboxStatus::Active =>
                {
                    Err(busy_or_corrupt(sandbox.status, sandbox.pause_eligibility()))
                }
                SandboxLifecycleCommandKind::Revive { .. }
                    if sandbox.status == SandboxStatus::Paused =>
                {
                    Err(busy_or_corrupt(
                        sandbox.status,
                        sandbox.revive_eligibility(),
                    ))
                }
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

fn busy_or_corrupt(
    status: SandboxStatus,
    eligibility: Result<(), SandboxLifecycleBlocker>,
) -> SandboxLifecycleCommandError {
    match eligibility {
        Err(reason) => SandboxLifecycleCommandError::SandboxBusy { status, reason },
        Ok(()) => SandboxLifecycleCommandError::Corrupt(
            "eligible sandbox could not begin its lifecycle transition".into(),
        ),
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
        SandboxStoreError::LifecycleBusy { status, reason } => {
            SandboxLifecycleCommandError::SandboxBusy { status, reason }
        }
        SandboxStoreError::Corrupt(detail) => SandboxLifecycleCommandError::Corrupt(detail),
        other => SandboxLifecycleCommandError::Unavailable(other.to_string()),
    }
}
