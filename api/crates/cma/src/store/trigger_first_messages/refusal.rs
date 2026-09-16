use super::*;
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
    super::super::sandbox_mongo::start_transaction(&mut session)
        .await
        .map_err(|_| CommandLedgerError::Unavailable)?;
    let result = apply(store, expected, revision, code, now, &mut session).await;
    if let Err(error) = result {
        let _ = session.abort_transaction().await;
        return Err(error);
    }
    match super::super::sandbox_mongo::commit_transaction(&mut session).await {
        super::super::sandbox_mongo::CommitOutcome::Committed => Ok(()),
        _ => Err(CommandLedgerError::Unavailable),
    }
}

async fn apply(
    store: &MongoSandboxStore,
    expected: &AdmittedAgentCommand,
    revision: u64,
    code: CommandFailureCode,
    now: String,
    session: &mut ClientSession,
) -> Result<(), CommandLedgerError> {
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
    project_terminal(store, &current, session)
        .await
        .map_err(|_| CommandLedgerError::Conflict)?;
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
    Ok(())
}
