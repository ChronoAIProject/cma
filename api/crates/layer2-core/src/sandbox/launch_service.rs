//! Trigger Launch-only sandbox recovery commands.

use crate::{now_rfc3339, Caller, Capability, SandboxId};

use super::launch_recovery::valid_launch_id;
use super::store::{
    SandboxLaunchCommand, SandboxLaunchCommandKind, SandboxLaunchCommandResult, SandboxStoreError,
};
use super::{SandboxLaunchCommandOutcome, SandboxService, SandboxServiceError};

impl SandboxService {
    pub async fn repair_service_grant_for_launch(
        &self,
        caller: &Caller,
        id: &SandboxId,
        launch_id: &str,
    ) -> Result<SandboxLaunchCommandOutcome, SandboxServiceError> {
        require_launch_authority(caller)?;
        validate_launch_id(launch_id)?;
        self.schedule_launch_command(SandboxLaunchCommand::repair_service_grant(
            id.clone(),
            caller.subject.clone(),
            launch_id.to_string(),
            now_rfc3339(),
        ))
        .await
    }

    pub async fn revive_for_launch(
        &self,
        caller: &Caller,
        id: &SandboxId,
        launch_id: &str,
        reused: bool,
    ) -> Result<SandboxLaunchCommandOutcome, SandboxServiceError> {
        require_launch_authority(caller)?;
        validate_launch_id(launch_id)?;
        self.require_connections(caller, id).await?;
        self.require_platform_open(crate::admission::AdmissionCause::Continuation)
            .await
            .map_err(SandboxServiceError::from)?;
        self.schedule_launch_command(SandboxLaunchCommand::revive(
            id.clone(),
            caller.subject.clone(),
            launch_id.to_string(),
            now_rfc3339(),
            reused,
        ))
        .await
    }

    async fn schedule_launch_command(
        &self,
        command: SandboxLaunchCommand,
    ) -> Result<SandboxLaunchCommandOutcome, SandboxServiceError> {
        let kind = command.kind;
        let launch_id = command.launch_id.clone();
        let sandbox_id = command.sandbox_id.clone();
        let result = self
            .store
            .schedule_sandbox_launch_command(command)
            .await
            .map_err(launch_store_error)?;
        match result {
            SandboxLaunchCommandResult::Accepted(outcome) => {
                tracing::info!(
                    sandbox_id = %sandbox_id,
                    launch_id,
                    command = launch_command_name(kind),
                    disposition = ?outcome.disposition,
                    "resolved sandbox launch recovery intent"
                );
                Ok(outcome)
            }
            SandboxLaunchCommandResult::OriginMismatch => {
                Err(SandboxServiceError::SandboxOriginConflict)
            }
            SandboxLaunchCommandResult::NotEligible(_) => Err(SandboxServiceError::InvalidState),
        }
    }
}

fn require_launch_authority(caller: &Caller) -> Result<(), SandboxServiceError> {
    (caller.authorization.allows(Capability::LaunchTriggers)
        || caller.authorization.allows(Capability::CreateSandbox))
    .then_some(())
    .ok_or(SandboxServiceError::Forbidden)
}

fn validate_launch_id(launch_id: &str) -> Result<(), SandboxServiceError> {
    valid_launch_id(launch_id)
        .then_some(())
        .ok_or(SandboxServiceError::InvalidOrigin)
}

fn launch_command_name(kind: SandboxLaunchCommandKind) -> &'static str {
    match kind {
        SandboxLaunchCommandKind::RepairServiceGrant => "repair_service_grant",
        SandboxLaunchCommandKind::Revive { reused: false } => "revive_created_sandbox",
        SandboxLaunchCommandKind::Revive { reused: true } => "revive_reused_sandbox",
    }
}

fn launch_store_error(error: SandboxStoreError) -> SandboxServiceError {
    match error {
        SandboxStoreError::NotFound => SandboxServiceError::NotFound,
        SandboxStoreError::InvalidTransaction(_) => SandboxServiceError::InvalidOrigin,
        SandboxStoreError::Corrupt(detail) => SandboxServiceError::Corrupt(detail),
        other => SandboxServiceError::Unavailable(other.to_string()),
    }
}
