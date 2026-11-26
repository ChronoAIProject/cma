use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::agent_io::{
    AdmittedInterruptingMessage, CommandLedgerError, CommandReservationResult,
    InterruptingMessageReservation,
};
use mongodb::{bson::doc, ClientSession};

use super::{records, reserve, retry::CommandAdmissionRetries, targeted_control, ApplyError};
use crate::store::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
    MongoSandboxStore,
};

pub(super) async fn run(
    store: &MongoSandboxStore,
    request: InterruptingMessageReservation,
) -> Result<AdmittedInterruptingMessage, CommandLedgerError> {
    request.validate()?;
    let mut retries = CommandAdmissionRetries::with_deadline(
        request.control.command.authority_deadline,
        request.control.retry_deadline,
    );
    while retries.next().await {
        request.require_current_authority()?;
        // `reserve_in_session` composes the control and message reservations
        // in this transaction, so the outer admission owns the Agent lock.
        let _writer = if store.agent_control_write_coordination {
            match store
                .agent_writes
                .lock_until(
                    &request.control.command.envelope.agent_id,
                    retries.deadline(),
                )
                .await
            {
                Some(writer) => Some(writer),
                None => break,
            }
        } else {
            None
        };
        request.require_current_authority()?;
        let mut session = store
            .client
            .start_session()
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
        start_transaction(&mut session)
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
        let result = match apply(store, &request, &mut session).await {
            Ok(result) => result,
            Err(ApplyError::Domain(error)) => {
                let _ = session.abort_guarded().await;
                return Err(error);
            }
            Err(ApplyError::Driver(error)) => {
                let _ = session.abort_guarded().await;
                if is_transient(&error) || is_duplicate_key(&error) {
                    continue;
                }
                return Err(CommandLedgerError::Unavailable);
            }
        };
        if let Err(error) = request.require_current_authority() {
            let _ = session.abort_guarded().await;
            return Err(error);
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(result),
            CommitOutcome::Retry(_) => continue,
            // The same original key recovers both records after ambiguity.
            CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                return Err(CommandLedgerError::Unavailable)
            }
        }
    }
    Err(CommandLedgerError::Conflict)
}

async fn apply(
    store: &MongoSandboxStore,
    request: &InterruptingMessageReservation,
    session: &mut ClientSession,
) -> Result<AdmittedInterruptingMessage, ApplyError> {
    let control = store
        .agent_commands
        .find_one(doc! {
            "scope": request.control.command.scope(),
            "key_sha256": &request.control.command.key_sha256,
        })
        .session(&mut *session)
        .await?;
    let message = store
        .agent_commands
        .find_one(doc! {
            "scope": request.message.scope(), "key_sha256": &request.message.key_sha256,
        })
        .session(&mut *session)
        .await?;
    // Neither legacy half can be upgraded into a pair. Compare immutable
    // intent before current target checks, including after the turn has ended.
    let existing = match (control, message) {
        (Some(control), Some(message)) => {
            let binding = control
                .get("explicit_control")
                .map(|value| {
                    mongodb::bson::deserialize_from_bson(value.clone())
                        .map_err(|_| CommandLedgerError::Corrupt)
                })
                .transpose()?;
            request.replay_records(records::read(&control)?, binding, records::read(&message)?)?;
            true
        }
        (None, None) => false,
        _ => return Err(CommandLedgerError::IdempotencyConflict.into()),
    };
    // Replay still writes current resource fences and verifies its original
    // response binding. Fresh creation also fences the still-active source.
    let control = targeted_control::reserve_in_session(store, &request.control, session).await?;
    // The control consumes the preceding admission position and capacity slot
    // in this same transaction. A full queue or failed message write rolls back
    // the control, its sidecar, both audits and every admission fence.
    // Historical replay also checks the replacement's live binding authority.
    let message = match (
        existing,
        reserve::apply(store, &request.message, session).await?,
    ) {
        (false, CommandReservationResult::Created(message))
        | (true, CommandReservationResult::Existing(message)) => message,
        _ => return Err(CommandLedgerError::IdempotencyConflict.into()),
    };
    Ok(request.replay(control, *message)?)
}
