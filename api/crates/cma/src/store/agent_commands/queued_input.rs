use layer2_core::sandbox::agent_io::{
    CommandLedgerError, QueuedInputOutcome, QueuedInputReservation,
};

use super::ApplyError;
use crate::store::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
    MongoSandboxStore, TRANSACTION_ATTEMPTS,
};

mod apply;

pub(super) async fn run(
    store: &MongoSandboxStore,
    request: QueuedInputReservation,
) -> Result<QueuedInputOutcome, CommandLedgerError> {
    request.validate()?;
    for _ in 0..TRANSACTION_ATTEMPTS {
        request.cancellation.command.require_current_authority()?;
        let mut session = store
            .client
            .start_session()
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
        start_transaction(&mut session)
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
        let result = match apply::apply(store, &request, &mut session).await {
            Ok(result) => result,
            Err(ApplyError::Domain(error)) => {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            Err(ApplyError::Driver(error)) => {
                let _ = session.abort_transaction().await;
                if is_transient(&error) || is_duplicate_key(&error) {
                    tokio::task::yield_now().await;
                    continue;
                }
                return Err(CommandLedgerError::Unavailable);
            }
        };
        if let Err(error) = request.cancellation.command.require_current_authority() {
            let _ = session.abort_transaction().await;
            return Err(error);
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(result),
            CommitOutcome::Retry(_) => continue,
            CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                return Err(CommandLedgerError::Unavailable)
            }
        }
    }
    Err(CommandLedgerError::Conflict)
}

/// Immutable receipt lookup after an uncertain peer reply. No transaction,
/// owner acquisition, resource touch, reservation or native side effect.
pub(super) async fn lookup(
    store: &MongoSandboxStore,
    request: QueuedInputReservation,
) -> Result<Option<layer2_core::sandbox::agent_io::AdmittedQueuedInput>, CommandLedgerError> {
    use mongodb::bson::doc;
    request.validate()?;
    request.cancellation.command.require_current_authority()?;
    let Some(document) = store
        .agent_commands
        .find_one(doc! {
            "scope": request.cancellation.command.scope(),
            "key_sha256": &request.cancellation.command.key_sha256,
        })
        .await
        .map_err(|_| CommandLedgerError::Unavailable)?
    else {
        return Ok(None);
    };
    let record = super::records::read(&document)?;
    let binding: layer2_core::sandbox::agent_io::QueuedInputBinding = document
        .get("queued_input_edit")
        .ok_or(CommandLedgerError::IdempotencyConflict)
        .and_then(|value| {
            mongodb::bson::deserialize_from_bson(value.clone())
                .map_err(|_| CommandLedgerError::Corrupt)
        })?;
    let response = store
        .agent_commands
        .find_one(doc! {"_id": binding.target.source_command_id.as_str()})
        .await
        .map_err(|_| CommandLedgerError::Unavailable)?
        .as_ref()
        .map(super::records::read)
        .transpose()?
        .ok_or(CommandLedgerError::Corrupt)?;
    request.cancellation.command.require_current_authority()?;
    request.replay(record, Some(binding), response).map(Some)
}
