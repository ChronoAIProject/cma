use crate::store::transaction_terminal::GuardedTerminal as _;
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
        // Local Agent writers already use this scheduler. Waiting here keeps
        // the control transaction from repeatedly losing the same document
        // lock while preserving the existing authority/retry deadline.
        let _writer = if store.agent_control_write_coordination {
            match store
                .agent_writes
                .lock_until(&request.command.envelope.agent_id, retries.deadline())
                .await
            {
                Some(writer) => Some(writer),
                None => break,
            }
        } else {
            None
        };
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
                let _ = session.abort_guarded().await;
                record_outcome("other_refused", attempt);
                return Err(error);
            }
            Err(ApplyError::Driver(error)) => {
                let _ = session.abort_guarded().await;
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
                record_outcome("other_refused", attempt);
                return Err(CommandLedgerError::Unavailable);
            }
        };
        if let Err(error) = request.command.require_current_authority() {
            let _ = session.abort_guarded().await;
            record_outcome("other_refused", attempt);
            return Err(error);
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => {
                let outcome = match &result {
                    TargetedControlInspection::NotAdmitted => "not_admitted",
                    TargetedControlInspection::Existing(_) | TargetedControlInspection::Missing => {
                        "committed"
                    }
                };
                record_outcome(outcome, attempt);
                return Ok(result);
            }
            CommitOutcome::Retry(error) => {
                trace_rejection(&request, "commit", ApplyError::Driver(error));
                continue;
            }
            // Same-key recovery reads the command and sidecar atomically even
            // after the original response completes. No runtime IO occurred.
            CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                record_outcome("other_refused", attempt);
                return Err(CommandLedgerError::Unavailable);
            }
        }
    }
    tracing::warn!(
        agent_id = %request.command.envelope.agent_id,
        response_id = %request.target.response_id,
        ?mode,
        "response control transaction retries exhausted"
    );
    record_outcome("conflict_exhausted", attempt);
    Err(CommandLedgerError::Conflict)
}

fn record_outcome(outcome: &'static str, attempts: u32) {
    metrics::counter!("cma_agent_control_admission_total", "outcome" => outcome).increment(1);
    metrics::histogram!("cma_agent_control_admission_attempts").record(attempts as f64);
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
