//! Atomic admission for owner and system sandbox lifecycle requests.

use async_trait::async_trait;

use crate::ids::{SandboxId, SubjectId};
use crate::seconds_from_rfc3339;

use super::{SandboxStore, SandboxStoreError, SandboxTransaction, SandboxTransactionStore};
use crate::sandbox::{
    Sandbox, SandboxEventKind, SandboxLifecycleOperation, SandboxReviveCause, SandboxStatus,
};

const COMMAND_CAS_ATTEMPTS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxLifecycleCommandKind {
    Pause { notice: Option<String> },
    Revive { cause: SandboxReviveCause },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxLifecycleCommand {
    /// Optional authority fence for a system decision made from one snapshot.
    pub expected_revision: Option<u64>,
    pub managed_runtime_version: Option<String>,
    pub sandbox_id: SandboxId,
    pub owner: SubjectId,
    pub requested_at: String,
    pub kind: SandboxLifecycleCommandKind,
}

impl SandboxLifecycleCommand {
    pub fn at_revision(mut self, revision: u64) -> Self {
        self.expected_revision = Some(revision);
        self
    }

    pub fn pause(
        sandbox_id: SandboxId,
        owner: SubjectId,
        requested_at: String,
        notice: Option<String>,
    ) -> Self {
        Self {
            expected_revision: None,
            managed_runtime_version: None,
            sandbox_id,
            owner,
            requested_at,
            kind: SandboxLifecycleCommandKind::Pause { notice },
        }
    }

    pub fn revive(
        sandbox_id: SandboxId,
        owner: SubjectId,
        requested_at: String,
        cause: SandboxReviveCause,
    ) -> Self {
        Self {
            expected_revision: None,
            managed_runtime_version: None,
            sandbox_id,
            owner,
            requested_at,
            kind: SandboxLifecycleCommandKind::Revive { cause },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxLifecycleCommandDisposition {
    Scheduled,
    AlreadyScheduled,
    AlreadySatisfied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxLifecycleCommandOutcome {
    pub sandbox: Sandbox,
    pub disposition: SandboxLifecycleCommandDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxLifecycleCommandResult {
    Accepted(SandboxLifecycleCommandOutcome),
    NotEligible(Sandbox),
}

/// The command is one revision-CAS plus its gap-free status event.
#[async_trait]
pub trait SandboxLifecycleCommandStore: Send + Sync {
    async fn schedule_sandbox_lifecycle(
        &self,
        command: SandboxLifecycleCommand,
    ) -> Result<SandboxLifecycleCommandResult, SandboxStoreError>;
}

#[async_trait]
impl<T> SandboxLifecycleCommandStore for T
where
    T: SandboxStore + SandboxTransactionStore + Send + Sync,
{
    async fn schedule_sandbox_lifecycle(
        &self,
        command: SandboxLifecycleCommand,
    ) -> Result<SandboxLifecycleCommandResult, SandboxStoreError> {
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
            let application = apply_sandbox_lifecycle_command(current, &command)?;
            let SandboxLifecycleCommandApplication::Commit {
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
                    return Ok(SandboxLifecycleCommandResult::Accepted(
                        SandboxLifecycleCommandOutcome {
                            sandbox,
                            disposition: SandboxLifecycleCommandDisposition::Scheduled,
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

/// Pure lifecycle admission for stores that atomically persist additional authority.
#[doc(hidden)]
pub enum SandboxLifecycleCommandApplication {
    Commit {
        expected_revision: u64,
        sandbox: Sandbox,
        event: SandboxEventKind,
    },
    Accepted(SandboxLifecycleCommandOutcome),
    NotEligible(Sandbox),
}

impl SandboxLifecycleCommandApplication {
    pub(super) fn result(self) -> SandboxLifecycleCommandResult {
        match self {
            Self::Accepted(outcome) => SandboxLifecycleCommandResult::Accepted(outcome),
            Self::NotEligible(sandbox) => SandboxLifecycleCommandResult::NotEligible(sandbox),
            Self::Commit { .. } => unreachable!("commits are handled before result projection"),
        }
    }
}

/// Validate and apply lifecycle policy without performing storage effects.
#[doc(hidden)]
pub fn apply_sandbox_lifecycle_command(
    current: Sandbox,
    command: &SandboxLifecycleCommand,
) -> Result<SandboxLifecycleCommandApplication, SandboxStoreError> {
    validate_command(command)?;
    if current.id != command.sandbox_id || current.owner != command.owner {
        return Err(SandboxStoreError::NotFound);
    }
    if command
        .expected_revision
        .is_some_and(|revision| revision != current.revision)
    {
        return Err(SandboxStoreError::Conflict);
    }
    current
        .validate()
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    match &command.kind {
        SandboxLifecycleCommandKind::Pause { notice } => apply_pause(current, command, notice),
        SandboxLifecycleCommandKind::Revive { cause } => apply_revive(current, command, *cause),
    }
}

fn apply_pause(
    current: Sandbox,
    command: &SandboxLifecycleCommand,
    notice: &Option<String>,
) -> Result<SandboxLifecycleCommandApplication, SandboxStoreError> {
    if current.status == SandboxStatus::Pausing
        && matches!(
            current.lifecycle.operation,
            Some(SandboxLifecycleOperation::Pause { .. })
        )
    {
        return Ok(accepted(
            current,
            SandboxLifecycleCommandDisposition::AlreadyScheduled,
        ));
    }
    if current.status == SandboxStatus::Paused {
        return Ok(accepted(
            current,
            SandboxLifecycleCommandDisposition::AlreadySatisfied,
        ));
    }
    if current.status != SandboxStatus::Active {
        return Ok(SandboxLifecycleCommandApplication::NotEligible(current));
    }
    let expected_revision = current.revision;
    let previous = current.status;
    let mut sandbox = current.clone();
    if sandbox
        .begin_pause(command.requested_at.clone(), notice.clone())
        .is_err()
    {
        return Ok(SandboxLifecycleCommandApplication::NotEligible(current));
    }
    validate_candidate(&sandbox)?;
    Ok(SandboxLifecycleCommandApplication::Commit {
        expected_revision,
        event: SandboxEventKind::StatusChanged {
            from: previous,
            to: SandboxStatus::Pausing,
        },
        sandbox,
    })
}

fn apply_revive(
    current: Sandbox,
    command: &SandboxLifecycleCommand,
    cause: SandboxReviveCause,
) -> Result<SandboxLifecycleCommandApplication, SandboxStoreError> {
    if current.status == SandboxStatus::Pausing {
        let mut sandbox = current.clone();
        if sandbox
            .request_pause_cancellation(command.requested_at.clone())
            .is_ok()
        {
            validate_candidate(&sandbox)?;
            return Ok(SandboxLifecycleCommandApplication::Commit {
                expected_revision: current.revision,
                sandbox,
                event: SandboxEventKind::WorkStepCompleted {
                    step: crate::sandbox::SandboxWorkStep::RunLifecycleOperation,
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
                SandboxLifecycleCommandDisposition::AlreadyScheduled,
            ));
        }
        return Ok(SandboxLifecycleCommandApplication::NotEligible(current));
    }
    if current.status == SandboxStatus::Reviving
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
            return Ok(SandboxLifecycleCommandApplication::Commit {
                expected_revision: current.revision,
                sandbox,
                event: SandboxEventKind::WorkStepCompleted {
                    step: crate::sandbox::SandboxWorkStep::RunLifecycleOperation,
                },
            });
        }
        return Ok(accepted(
            current,
            SandboxLifecycleCommandDisposition::AlreadyScheduled,
        ));
    }
    if current.status == SandboxStatus::Active {
        return Ok(accepted(
            current,
            SandboxLifecycleCommandDisposition::AlreadySatisfied,
        ));
    }
    if matches!(
        current.status,
        SandboxStatus::Provisioning
            | SandboxStatus::Installing
            | SandboxStatus::CredentialRotating
            | SandboxStatus::Rotating
    ) {
        return Ok(accepted(
            current,
            SandboxLifecycleCommandDisposition::AlreadyScheduled,
        ));
    }
    if current.status != SandboxStatus::Paused {
        return Ok(SandboxLifecycleCommandApplication::NotEligible(current));
    }
    let expected_revision = current.revision;
    let previous = current.status;
    let mut sandbox = current.clone();
    if sandbox
        .begin_revive_with_managed_runtime(
            cause,
            command.managed_runtime_version.as_deref(),
            command.requested_at.clone(),
        )
        .is_err()
    {
        return Ok(SandboxLifecycleCommandApplication::NotEligible(current));
    }
    validate_candidate(&sandbox)?;
    Ok(SandboxLifecycleCommandApplication::Commit {
        expected_revision,
        event: SandboxEventKind::StatusChanged {
            from: previous,
            to: SandboxStatus::Reviving,
        },
        sandbox,
    })
}

fn accepted(
    sandbox: Sandbox,
    disposition: SandboxLifecycleCommandDisposition,
) -> SandboxLifecycleCommandApplication {
    SandboxLifecycleCommandApplication::Accepted(SandboxLifecycleCommandOutcome {
        sandbox,
        disposition,
    })
}

fn validate_command(command: &SandboxLifecycleCommand) -> Result<(), SandboxStoreError> {
    if command.sandbox_id.as_str().trim().is_empty()
        || command.owner.as_str().trim().is_empty()
        || seconds_from_rfc3339(&command.requested_at).is_none()
    {
        return Err(SandboxStoreError::InvalidTransaction(
            "sandbox lifecycle command is invalid",
        ));
    }
    Ok(())
}

fn validate_candidate(sandbox: &Sandbox) -> Result<(), SandboxStoreError> {
    sandbox.validate().map_err(|error| {
        SandboxStoreError::Corrupt(format!("scheduled sandbox is invalid: {error}"))
    })
}
