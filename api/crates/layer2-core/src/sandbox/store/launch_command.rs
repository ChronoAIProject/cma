//! Atomic, origin-bound commands used by Trigger Launch orchestration.

use async_trait::async_trait;

use crate::ids::{SandboxId, SubjectId};
use crate::sandbox::launch_recovery::valid_launch_id;
use crate::seconds_from_rfc3339;

use super::{SandboxStore, SandboxStoreError, SandboxTransaction, SandboxTransactionStore};
use crate::sandbox::{
    Sandbox, SandboxEventKind, SandboxLifecycleOperation, SandboxReviveCause, SandboxStatus,
    SandboxWorkStep,
};

const COMMAND_CAS_ATTEMPTS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxLaunchCommandKind {
    RepairServiceGrant,
    Revive { reused: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxLaunchCommand {
    pub managed_runtime_version: Option<String>,
    pub sandbox_id: SandboxId,
    pub owner: SubjectId,
    pub launch_id: String,
    pub requested_at: String,
    pub kind: SandboxLaunchCommandKind,
}

impl SandboxLaunchCommand {
    pub fn repair_service_grant(
        sandbox_id: SandboxId,
        owner: SubjectId,
        launch_id: String,
        requested_at: String,
    ) -> Self {
        Self {
            managed_runtime_version: None,
            sandbox_id,
            owner,
            launch_id,
            requested_at,
            kind: SandboxLaunchCommandKind::RepairServiceGrant,
        }
    }

    pub fn revive(
        sandbox_id: SandboxId,
        owner: SubjectId,
        launch_id: String,
        requested_at: String,
        reused: bool,
    ) -> Self {
        Self {
            managed_runtime_version: None,
            sandbox_id,
            owner,
            launch_id,
            requested_at,
            kind: SandboxLaunchCommandKind::Revive { reused },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxLaunchCommandDisposition {
    Scheduled,
    AlreadyScheduled,
    AlreadySatisfied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxLaunchCommandOutcome {
    pub sandbox: Sandbox,
    pub disposition: SandboxLaunchCommandDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxLaunchCommandResult {
    Accepted(SandboxLaunchCommandOutcome),
    OriginMismatch,
    NotEligible(Sandbox),
}

#[async_trait]
pub trait SandboxLaunchCommandStore: Send + Sync {
    async fn schedule_sandbox_launch_command(
        &self,
        command: SandboxLaunchCommand,
    ) -> Result<SandboxLaunchCommandResult, SandboxStoreError>;
}

#[async_trait]
impl<T> SandboxLaunchCommandStore for T
where
    T: SandboxStore + SandboxTransactionStore + Send + Sync,
{
    async fn schedule_sandbox_launch_command(
        &self,
        command: SandboxLaunchCommand,
    ) -> Result<SandboxLaunchCommandResult, SandboxStoreError> {
        let mut command = command;
        command.managed_runtime_version = self.managed_runtime_version();
        validate_command(&command)?;
        for _ in 0..COMMAND_CAS_ATTEMPTS {
            let release = self.capture_released_runtime();
            let current = self
                .get_sandbox(&command.sandbox_id)
                .await?
                .filter(|sandbox| sandbox.owner == command.owner)
                .ok_or(SandboxStoreError::NotFound)?;
            command.managed_runtime_version = release
                .as_ref()
                .and_then(|release| {
                    self.managed_runtime_for_release(&current, release)
                        .map(|(_, target)| target)
                })
                .map(str::to_owned);
            current.validate().map_err(|error| {
                SandboxStoreError::Corrupt(format!("stored sandbox is invalid: {error}"))
            })?;
            if !origin_matches(&current, &command) {
                return Ok(SandboxLaunchCommandResult::OriginMismatch);
            }
            let application = apply_command(current, &command)?;
            let PendingApplication::Commit {
                expected_revision,
                sandbox,
                event,
            } = application
            else {
                return Ok(application.result());
            };
            match self
                .commit(SandboxTransaction::SaveSandboxAndAppend {
                    command_revival: None,
                    released_runtime: release,
                    expected_revision,
                    sandbox: Box::new(sandbox.clone()),
                    event_at: command.requested_at.clone(),
                    event,
                })
                .await
            {
                Ok(()) => {
                    return Ok(SandboxLaunchCommandResult::Accepted(
                        SandboxLaunchCommandOutcome {
                            sandbox,
                            disposition: SandboxLaunchCommandDisposition::Scheduled,
                        },
                    ));
                }
                Err(SandboxStoreError::Conflict | SandboxStoreError::SequenceConflict) => {
                    tokio::task::yield_now().await;
                }
                Err(error) => return Err(error),
            }
        }
        Err(SandboxStoreError::Conflict)
    }
}

enum PendingApplication {
    Commit {
        expected_revision: u64,
        sandbox: Sandbox,
        event: SandboxEventKind,
    },
    Accepted(SandboxLaunchCommandOutcome),
    NotEligible(Sandbox),
}

impl PendingApplication {
    fn result(self) -> SandboxLaunchCommandResult {
        match self {
            Self::Accepted(outcome) => SandboxLaunchCommandResult::Accepted(outcome),
            Self::NotEligible(sandbox) => SandboxLaunchCommandResult::NotEligible(sandbox),
            Self::Commit { .. } => unreachable!("commits are handled before result projection"),
        }
    }
}

fn apply_command(
    current: Sandbox,
    command: &SandboxLaunchCommand,
) -> Result<PendingApplication, SandboxStoreError> {
    match command.kind {
        SandboxLaunchCommandKind::RepairServiceGrant => apply_repair(current, command),
        SandboxLaunchCommandKind::Revive { .. } => apply_revive(current, command),
    }
}

fn apply_repair(
    current: Sandbox,
    command: &SandboxLaunchCommand,
) -> Result<PendingApplication, SandboxStoreError> {
    if current.is_post_install_repair_replay() {
        let disposition = if current.status == SandboxStatus::Active {
            SandboxLaunchCommandDisposition::AlreadySatisfied
        } else {
            SandboxLaunchCommandDisposition::AlreadyScheduled
        };
        return Ok(accepted(current, disposition));
    }
    if !current.service_grant_failure_is_repairable() {
        return Ok(PendingApplication::NotEligible(current));
    }
    let expected_revision = current.revision;
    let mut sandbox = current.clone();
    sandbox
        .repair_service_grant_failure(&command.launch_id, command.requested_at.clone())
        .map_err(|error| {
            SandboxStoreError::Corrupt(format!("repaired sandbox is invalid: {error}"))
        })?;
    Ok(PendingApplication::Commit {
        expected_revision,
        sandbox,
        event: SandboxEventKind::StatusChanged {
            from: SandboxStatus::Failed,
            to: SandboxStatus::Installing,
        },
    })
}

fn apply_revive(
    current: Sandbox,
    command: &SandboxLaunchCommand,
) -> Result<PendingApplication, SandboxStoreError> {
    if current.status == SandboxStatus::Pausing {
        let mut sandbox = current.clone();
        if sandbox
            .request_pause_cancellation(command.requested_at.clone())
            .is_ok()
        {
            return Ok(PendingApplication::Commit {
                expected_revision: current.revision,
                sandbox,
                event: SandboxEventKind::WorkStepCompleted {
                    step: SandboxWorkStep::RunLifecycleOperation,
                },
            });
        }
        if matches!(
            current.lifecycle.operation,
            Some(SandboxLifecycleOperation::Pause {
                phase: crate::sandbox::SandboxPausePhase::ResumeWriters,
                ..
            })
        ) {
            return Ok(accepted(
                current,
                SandboxLaunchCommandDisposition::AlreadyScheduled,
            ));
        }
        return Ok(PendingApplication::NotEligible(current));
    }
    if current.status == SandboxStatus::Reviving
        && current.work.step == SandboxWorkStep::RunLifecycleOperation
        && current.has_structurally_valid_work()
        && matches!(
            current.lifecycle.operation,
            Some(SandboxLifecycleOperation::Revive { .. })
        )
    {
        let mut sandbox = current.clone();
        if sandbox
            .cancel_recovery_pause(&command.requested_at)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?
        {
            return Ok(PendingApplication::Commit {
                expected_revision: current.revision,
                sandbox,
                event: SandboxEventKind::WorkStepCompleted {
                    step: SandboxWorkStep::RunLifecycleOperation,
                },
            });
        }
        return Ok(accepted(
            current,
            SandboxLaunchCommandDisposition::AlreadyScheduled,
        ));
    }
    if current.status == SandboxStatus::Active {
        return Ok(accepted(
            current,
            SandboxLaunchCommandDisposition::AlreadySatisfied,
        ));
    }
    if current.status != SandboxStatus::Paused {
        return Ok(PendingApplication::NotEligible(current));
    }
    let expected_revision = current.revision;
    let mut sandbox = current.clone();
    if sandbox
        .begin_revive_with_managed_runtime(
            SandboxReviveCause::TriggerLaunch,
            command.managed_runtime_version.as_deref(),
            command.requested_at.clone(),
        )
        .is_err()
    {
        return Ok(PendingApplication::NotEligible(current));
    }
    sandbox.validate().map_err(|error| {
        SandboxStoreError::Corrupt(format!("launch-revived sandbox is invalid: {error}"))
    })?;
    Ok(PendingApplication::Commit {
        expected_revision,
        sandbox,
        event: SandboxEventKind::StatusChanged {
            from: SandboxStatus::Paused,
            to: SandboxStatus::Reviving,
        },
    })
}

fn accepted(sandbox: Sandbox, disposition: SandboxLaunchCommandDisposition) -> PendingApplication {
    PendingApplication::Accepted(SandboxLaunchCommandOutcome {
        sandbox,
        disposition,
    })
}

fn origin_matches(sandbox: &Sandbox, command: &SandboxLaunchCommand) -> bool {
    match command.kind {
        SandboxLaunchCommandKind::RepairServiceGrant
        | SandboxLaunchCommandKind::Revive { reused: false } => {
            sandbox.has_exact_launch_origin(&command.launch_id)
        }
        SandboxLaunchCommandKind::Revive { reused: true } => {
            sandbox.has_structurally_valid_origin()
        }
    }
}

fn validate_command(command: &SandboxLaunchCommand) -> Result<(), SandboxStoreError> {
    if command.sandbox_id.as_str().trim().is_empty()
        || command.owner.as_str().trim().is_empty()
        || !valid_launch_id(&command.launch_id)
        || seconds_from_rfc3339(&command.requested_at).is_none()
    {
        return Err(SandboxStoreError::InvalidTransaction(
            "sandbox launch command is invalid",
        ));
    }
    Ok(())
}
