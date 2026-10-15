use layer2_core::sandbox::agent_io::{
    AdmittedTargetedControl, CommandLedgerError, TargetedControlInspection,
    TargetedControlReservation,
};

use super::ApplyError;
use crate::store::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
    MongoSandboxStore, TRANSACTION_ATTEMPTS,
};

mod apply;
mod fences;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Lookup,
    Reserve,
    Inspect,
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
    for attempt in 0..TRANSACTION_ATTEMPTS {
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
                        "response control transaction retry"
                    );
                    tokio::task::yield_now().await;
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
            CommitOutcome::Retry(_) => continue,
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
    if let ApplyError::Domain(ref reason) = error {
        tracing::warn!(
            agent_id = %request.command.envelope.agent_id,
            response_id = %request.target.response_id,
            expected_agent_revision = request.command.expected_agent_revision,
            observed_agent_revision = request.observed_agent_revision,
            stage,
            code = reason.code(),
            "response control admission refused"
        );
    }
    error
}
