use super::*;
use crate::store::agent_log_segments::{
    command_worker, terminal, transaction_scope::TransactionScope,
};
use layer2_core::sandbox::agent_io::{CommandFailureCode, CommandLedgerError};
use mongodb::ClientSession;

pub(in crate::store) async fn reject_unclaimed(
    store: &MongoSandboxStore,
    expected: &AdmittedAgentCommand,
    revision: u64,
    code: CommandFailureCode,
    now: String,
) -> Result<(), CommandLedgerError> {
    use crate::store::sandbox_mongo::{
        commit_transaction, is_transient, CommitOutcome, TRANSACTION_ATTEMPTS,
    };
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = store
            .client
            .start_session()
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
        let mut scope = TransactionScope::start(&mut session)
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
        let result = Box::pin(command_worker::refuse(
            store,
            expected,
            revision,
            code,
            now.clone(),
            &mut scope,
        ))
        .await;
        let session = scope.release();
        let receipt = match result {
            Ok(receipt) => receipt,
            Err(ApplyError::Domain(error)) => {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            Err(ApplyError::Driver(error)) => {
                let _ = session.abort_transaction().await;
                if is_transient(&error) {
                    tokio::task::yield_now().await;
                    continue;
                }
                return Err(CommandLedgerError::Unavailable);
            }
        };
        match commit_transaction(session).await {
            CommitOutcome::Committed => return Ok(()),
            CommitOutcome::Retry(_) => {
                tokio::task::yield_now().await;
            }
            CommitOutcome::Unknown(_) => {
                return if terminal::reconcile(store, receipt.as_ref(), session.operation_time())
                    .await?
                {
                    Ok(())
                } else {
                    Err(CommandLedgerError::Unavailable)
                };
            }
            CommitOutcome::Failed(_) => return Err(CommandLedgerError::Unavailable),
        }
    }
    Err(CommandLedgerError::Unavailable)
}

pub(in crate::store) async fn apply(
    store: &MongoSandboxStore,
    expected: &AdmittedAgentCommand,
    revision: u64,
    code: CommandFailureCode,
    now: String,
    session: &mut ClientSession,
) -> Result<AdmittedAgentCommand, ApplyError> {
    let row = store
        .agent_commands
        .find_one(doc! {"_id":expected.receipt.envelope.command_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(CommandLedgerError::NotFound)?;
    let mut current = super::records::read(&row)?;
    if current.receipt != expected.receipt || current.key_sha256 != expected.key_sha256 {
        return Err(CommandLedgerError::Corrupt.into());
    }
    current.reject_unclaimed(revision, code, now)?;
    let mut fields = super::records::document(&current)?;
    fields.remove("_id");
    let revision = super::super::sandbox_mongo::signed(revision, "command revision")
        .map_err(|_| CommandLedgerError::Invalid)?;
    let changed = store
        .agent_commands
        .update_one(
            doc! {"_id":current.receipt.envelope.command_id.as_str(),"revision":revision},
            doc! {"$set":fields},
        )
        .session(session)
        .await
        .map_err(ApplyError::Driver)?;
    if changed.matched_count != 1 {
        return Err(CommandLedgerError::StaleClaim.into());
    }
    Ok(current)
}
