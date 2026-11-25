use std::sync::Arc;

use super::lifecycle_command::{
    apply_sandbox_lifecycle_command, SandboxLifecycleCommandApplication,
};
use super::{
    SandboxLifecycleCommand, SandboxLifecycleCommandDisposition, SandboxLifecycleCommandOutcome,
    SandboxLifecycleCommandResult, SandboxStore, SandboxStoreError, SandboxTransaction,
    SandboxTransactionStore,
};
use crate::sandbox::agent_io::CommandRevivalAuthorization;
use crate::sandbox::{Sandbox, SandboxEventKind, SandboxReviveCause};

/// Scheduling performs no runtime IO. Once the transaction commits, cancellation
/// of the requesting worker cannot cancel or repeat the accepted lifecycle work.
#[async_trait::async_trait]
pub trait CommandRevivalStore: Send + Sync {
    async fn schedule_command_revival(
        &self,
        authorization: Arc<CommandRevivalAuthorization>,
    ) -> Result<SandboxLifecycleCommandResult, SandboxStoreError>;
}

#[async_trait::async_trait]
impl<T: SandboxStore + SandboxTransactionStore + Send + Sync> CommandRevivalStore for T {
    async fn schedule_command_revival(
        &self,
        authorization: Arc<CommandRevivalAuthorization>,
    ) -> Result<SandboxLifecycleCommandResult, SandboxStoreError> {
        for _ in 0..16 {
            let snapshot = authorization
                .current_snapshot()
                .await
                .map_err(|_| SandboxStoreError::Conflict)?;
            let current = snapshot.lineage.sandbox;
            current
                .validate()
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
            let at = crate::now_rfc3339();
            let release = self.capture_released_runtime();
            let command = revival(
                &current,
                &at,
                release
                    .as_ref()
                    .and_then(|release| self.managed_runtime_target(&current, &release.agent)),
            );
            let application = apply_sandbox_lifecycle_command(current, &command)?;
            let SandboxLifecycleCommandApplication::Commit {
                expected_revision,
                sandbox,
                event,
            } = application
            else {
                // This result has no lifecycle effect. Every mutation, including
                // cancellation of a pause, rechecks the actual ledger in commit.
                authorization
                    .require_current()
                    .map_err(|_| SandboxStoreError::Conflict)?;
                return Ok(application.result());
            };
            match self
                .commit(SandboxTransaction::SaveSandboxAndAppend {
                    command_revival: Some(authorization.clone()),
                    released_runtime: release,
                    expected_revision,
                    sandbox: Box::new(sandbox.clone()),
                    event_at: at,
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
                    ))
                }
                Err(SandboxStoreError::Conflict | SandboxStoreError::SequenceConflict) => {
                    self.refresh_released_runtime().await;
                    tokio::task::yield_now().await
                }
                Err(error) => return Err(error),
            }
        }
        Err(SandboxStoreError::Conflict)
    }
}

/// A command revival proof cannot be reused to pause, rotate credentials, change
/// services or otherwise edit the parent. Stores re-run the existing planner.
pub fn validate_command_revival_transition(
    current: &Sandbox,
    candidate: &Sandbox,
    at: &str,
    event: &SandboxEventKind,
    managed_runtime_version: Option<&str>,
) -> Result<(), SandboxStoreError> {
    current
        .validate()
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    let SandboxLifecycleCommandApplication::Commit {
        expected_revision,
        sandbox,
        event: expected_event,
    } = apply_sandbox_lifecycle_command(
        current.clone(),
        &revival(current, at, managed_runtime_version),
    )?
    else {
        return Err(SandboxStoreError::Conflict);
    };
    if expected_revision != current.revision || &sandbox != candidate || &expected_event != event {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
}

fn revival(
    current: &Sandbox,
    at: &str,
    managed_runtime_version: Option<&str>,
) -> SandboxLifecycleCommand {
    let mut command = SandboxLifecycleCommand::revive(
        current.id.clone(),
        current.owner.clone(),
        at.into(),
        SandboxReviveCause::AgentMessage,
    );
    command.managed_runtime_version = managed_runtime_version.map(str::to_owned);
    command
}
