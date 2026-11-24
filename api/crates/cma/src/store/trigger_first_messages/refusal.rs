use super::*;
use crate::store::agent_log_segments::{
    command_worker, terminal, transaction_scope::TransactionScope,
};
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::agent_io::{CommandFailureCode, CommandLedgerError};

pub(in crate::store) async fn reject_unclaimed(
    store: &MongoSandboxStore,
    expected: &AdmittedAgentCommand,
    revision: u64,
    code: CommandFailureCode,
    now: String,
) -> Result<(), CommandLedgerError> {
    let mut session = store
        .client
        .start_session()
        .await
        .map_err(|_| CommandLedgerError::Unavailable)?;
    let mut scope = TransactionScope::start(&mut session)
        .await
        .map_err(|_| CommandLedgerError::Unavailable)?;
    let result = Box::pin(command_worker::refuse(
        store, expected, revision, code, now, &mut scope,
    ))
    .await;
    let session = scope.release();
    let receipt = match result {
        Ok(receipt) => receipt,
        Err(error) => {
            let _ = session.abort_guarded().await;
            return Err(error);
        }
    };
    match super::super::sandbox_mongo::commit_transaction(session).await {
        super::super::sandbox_mongo::CommitOutcome::Committed => Ok(()),
        super::super::sandbox_mongo::CommitOutcome::Unknown(_) => {
            if terminal::reconcile(store, receipt.as_ref(), session.operation_time()).await? {
                Ok(())
            } else {
                Err(CommandLedgerError::Unavailable)
            }
        }
        _ => Err(CommandLedgerError::Unavailable),
    }
}

pub(in crate::store) async fn apply(
    store: &MongoSandboxStore,
    expected: &AdmittedAgentCommand,
    revision: u64,
    code: CommandFailureCode,
    now: String,
    session: &mut ClientSession,
) -> Result<AdmittedAgentCommand, CommandLedgerError> {
    let row = store
        .agent_commands
        .find_one(doc! {"_id":expected.receipt.envelope.command_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(|_| CommandLedgerError::Unavailable)?
        .ok_or(CommandLedgerError::NotFound)?;
    let mut current = super::super::agent_commands::read_command(&row)?;
    if current.receipt != expected.receipt || current.key_sha256 != expected.key_sha256 {
        return Err(CommandLedgerError::Corrupt);
    }
    current.reject_unclaimed(revision, code, now)?;
    let mut fields = super::super::agent_commands::command_document(&current)?;
    fields.remove("_id");
    let changed = store
        .agent_commands
        .update_one(
            doc! {"_id":current.receipt.envelope.command_id.as_str(),"revision":revision as i64},
            doc! {"$set":fields},
        )
        .session(session)
        .await
        .map_err(|_| CommandLedgerError::Unavailable)?;
    if changed.matched_count != 1 {
        return Err(CommandLedgerError::StaleClaim);
    }
    Ok(current)
}
