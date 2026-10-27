use layer2_core::sandbox::agent_io::{
    AdmittedTargetedControl, CommandLedgerError, TargetedControlInspection,
    TargetedControlReservation,
};
use mongodb::error::{ErrorKind, WriteFailure};

use super::{retry::CommandAdmissionRetries, ApplyError};
use crate::store::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
    MongoSandboxStore,
};

mod apply;
mod fences;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Lookup,
    Reserve,
    Inspect,
}

/// Compose targeted admission with another command without committing either
/// half independently. All existing target, authority and resource fences apply.
pub(super) async fn reserve_in_session(
    store: &MongoSandboxStore,
    request: &TargetedControlReservation,
    session: &mut mongodb::ClientSession,
) -> Result<AdmittedTargetedControl, ApplyError> {
    match apply::apply(store, request, Mode::Reserve, session).await? {
        TargetedControlInspection::Existing(value) => Ok(*value),
        _ => Err(CommandLedgerError::Corrupt.into()),
    }
}

pub(super) async fn run(
    store: &MongoSandboxStore,
    request: TargetedControlReservation,
    create: bool,
) -> Result<Option<AdmittedTargetedControl>, CommandLedgerError> {
    let mode = if create { Mode::Reserve } else { Mode::Lookup };
    Ok(match execute(store, request, mode).await? {
        TargetedControlInspection::Existing(existing) => Some(*existing),
        TargetedControlInspection::Missing | TargetedControlInspection::NotAdmitted => None,
    })
}

pub(super) async fn inspect(
    store: &MongoSandboxStore,
    request: TargetedControlReservation,
) -> Result<TargetedControlInspection, CommandLedgerError> {
    execute(store, request, Mode::Inspect).await
}

async fn execute(
    store: &MongoSandboxStore,
    request: TargetedControlReservation,
    mode: Mode,
) -> Result<TargetedControlInspection, CommandLedgerError> {
    request.validate()?;
    let mut retries = CommandAdmissionRetries::with_deadline(
        request.command.authority_deadline,
        request.retry_deadline,
    );
    let mut attempt = 0_u32;
    while retries.next().await {
        attempt = attempt.saturating_add(1);
        request.command.require_current_authority()?;
        let mut session = store
            .client
            .start_session()
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
        start_transaction(&mut session)
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
        let result = match apply::apply(store, &request, mode, &mut session).await {
            Ok(result) => result,
            Err(ApplyError::Domain(error)) => {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            Err(ApplyError::Driver(error)) => {
                let _ = session.abort_transaction().await;
                if is_transient(&error) || is_duplicate_key(&error) {
                    tracing::warn!(
                        agent_id = %request.command.envelope.agent_id,
                        response_id = %request.target.response_id,
                        ?mode, attempt,
                        transient = is_transient(&error),
                        duplicate_key = is_duplicate_key(&error),
                        database_code = database_code(&error),
                        "response control transaction retry"
                    );
                    continue;
                }
                return Err(CommandLedgerError::Unavailable);
            }
        };
        if let Err(error) = request.command.require_current_authority() {
            let _ = session.abort_transaction().await;
            return Err(error);
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(result),
            CommitOutcome::Retry(error) => {
                trace_rejection(&request, "commit", ApplyError::Driver(error));
                continue;
            }
            // Same-key recovery reads the command and sidecar atomically even
            // after the original response completes. No runtime IO occurred.
            CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                return Err(CommandLedgerError::Unavailable)
            }
        }
    }
    tracing::warn!(
        agent_id = %request.command.envelope.agent_id,
        response_id = %request.target.response_id,
        ?mode,
        "response control transaction retries exhausted"
    );
    Err(CommandLedgerError::Conflict)
}

fn trace_rejection(
    request: &TargetedControlReservation,
    stage: &'static str,
    error: ApplyError,
) -> ApplyError {
    match &error {
        ApplyError::Domain(reason) => tracing::warn!(
            agent_id = %request.command.envelope.agent_id,
            response_id = %request.target.response_id,
            expected_agent_revision = request.command.expected_agent_revision,
            observed_agent_revision = request.observed_agent_revision,
            stage,
            code = reason.code(),
            "response control admission refused"
        ),
        ApplyError::Driver(reason) => tracing::warn!(
            agent_id = %request.command.envelope.agent_id,
            response_id = %request.target.response_id,
            expected_agent_revision = request.command.expected_agent_revision,
            observed_agent_revision = request.observed_agent_revision,
            stage,
            transient = is_transient(reason),
            duplicate_key = is_duplicate_key(reason),
            database_code = database_code(reason),
            "response control admission database failure"
        ),
    }
    error
}

fn database_code(error: &mongodb::error::Error) -> Option<i32> {
    match error.kind.as_ref() {
        ErrorKind::Write(WriteFailure::WriteError(write)) => Some(write.code),
        ErrorKind::Command(command) => Some(command.code),
        _ => None,
    }
}
