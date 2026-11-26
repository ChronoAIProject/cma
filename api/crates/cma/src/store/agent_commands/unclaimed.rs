use mongodb::{bson::doc, ClientSession};

use super::{records, worker_apply, ApplyError};
use crate::store::agent_log_segments::{
    command_worker, terminal, transaction_scope::TransactionScope,
};
use crate::store::sandbox_mongo::{
    commit_transaction, is_transient, signed, CommitOutcome, MongoSandboxStore,
    TRANSACTION_ATTEMPTS,
};
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::agent_io::{
    AdmittedAgentCommand, CommandFailureCode, CommandLedgerError,
};

pub(super) async fn run(
    store: &MongoSandboxStore,
    expected: &AdmittedAgentCommand,
    revision: u64,
    code: CommandFailureCode,
    now: String,
) -> Result<(), CommandLedgerError> {
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
                let _ = session.abort_guarded().await;
                return Err(error);
            }
            Err(ApplyError::Driver(error)) => {
                let _ = session.abort_guarded().await;
                if is_transient(&error) {
                    continue;
                }
                return Err(CommandLedgerError::Unavailable);
            }
        };
        match commit_transaction(session).await {
            CommitOutcome::Committed => return Ok(()),
            CommitOutcome::Retry(_) => continue,
            CommitOutcome::Unknown(_) => {
                if terminal::reconcile(store, receipt.as_ref(), session.operation_time()).await? {
                    return Ok(());
                }
                return Err(CommandLedgerError::Unavailable);
            }
            CommitOutcome::Failed(_) => return Err(CommandLedgerError::Unavailable),
        }
    }
    Err(CommandLedgerError::Conflict)
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
        .await?
        .ok_or(CommandLedgerError::NotFound)?;
    let mut current = records::read(&row)?;
    if current.receipt != expected.receipt || current.key_sha256 != expected.key_sha256 {
        return Err(CommandLedgerError::Corrupt.into());
    }
    current.reject_unclaimed(revision, code, now)?;
    let agent =
        worker_apply::load_agent(store, &current.receipt.envelope.agent_id, session).await?;
    if !worker_apply::fence_parent(store, agent.as_ref(), session).await? {
        return Err(CommandLedgerError::StaleClaim.into());
    }
    let mut fields = records::document(&current)?;
    fields.remove("_id");
    let revision = signed(revision, "command revision").map_err(|_| CommandLedgerError::Invalid)?;
    let changed = store
        .agent_commands
        .update_one(
            doc! {"_id":current.receipt.envelope.command_id.as_str(),"revision":revision},
            doc! {"$set":fields},
        )
        .session(session)
        .await?;
    if changed.matched_count != 1 {
        return Err(CommandLedgerError::StaleClaim.into());
    }
    Ok(current)
}
