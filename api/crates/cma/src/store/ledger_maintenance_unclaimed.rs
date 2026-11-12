//! Settle an unclaimed command under its actual Agent's maintenance barrier.
//! This does not claim a lane or dispatch input. Unknown commits remain unknown.

use layer2_core::sandbox::agent_io::{
    AdmittedAgentCommand, CommandFailureCode, CommandLedgerError,
};
use layer2_core::sandbox::ledger_maintenance::LedgerWriterClass;
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::{bson::doc, ClientSession};

use super::agent_commands::ApplyError;
use super::agent_log_integrity::TransactionLogError;
use super::sandbox_mongo::{
    commit_transaction, is_transient, signed, start_transaction, CommitOutcome, MongoSandboxStore,
    TRANSACTION_ATTEMPTS,
};

pub(super) async fn reject(
    store: &MongoSandboxStore,
    expected: &AdmittedAgentCommand,
    revision: u64,
    code: CommandFailureCode,
    now: &str,
) -> Result<(), CommandLedgerError> {
    // Trigger settlement also projects its retained first-message receipt. Keep
    // that existing transaction; its separate hook uses `fence` below.
    if matches!(
        expected.receipt.envelope.actor,
        layer2_core::sandbox::agent_io::AgentActor::Trigger { .. }
    ) {
        return Err(CommandLedgerError::Invalid);
    }
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = store
            .client
            .start_session()
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
        start_transaction(&mut session)
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
        match apply(store, expected, revision, code, now, &mut session).await {
            Ok(()) => {}
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
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(()),
            CommitOutcome::Retry(_) => continue,
            CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                return Err(CommandLedgerError::Unavailable)
            }
        }
    }
    Err(CommandLedgerError::Conflict)
}

pub(super) async fn apply(
    store: &MongoSandboxStore,
    expected: &AdmittedAgentCommand,
    revision: u64,
    code: CommandFailureCode,
    now: &str,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let id = &expected.receipt.envelope.command_id;
    let row = store
        .agent_commands
        .find_one(doc! {"_id":id.as_str()})
        .session(&mut *session)
        .await?
        .ok_or(CommandLedgerError::NotFound)?;
    let mut current = super::agent_commands::read_command(&row)?;
    if current.receipt != expected.receipt || current.key_sha256 != expected.key_sha256 {
        return Err(CommandLedgerError::Corrupt.into());
    }
    // Preserve original revision/lease/dispatch/error-code decisions before the
    // parent fence. Refusals abort the entire transaction with no partial fence.
    current.reject_unclaimed(revision, code, now.into())?;
    fence(store, &current, session).await?;
    let mut fields = super::agent_commands::command_document(&current)?;
    fields.remove("_id");
    let revision = signed(revision, "command revision").map_err(|_| CommandLedgerError::Invalid)?;
    let changed = store
        .agent_commands
        .update_one(
            doc! {"_id":id.as_str(), "revision":revision},
            doc! {"$set":fields},
        )
        .session(session)
        .await?;
    if changed.matched_count != 1 {
        return Err(CommandLedgerError::StaleClaim.into());
    }
    Ok(())
}

pub(super) async fn fence(
    store: &MongoSandboxStore,
    command: &AdmittedAgentCommand,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let id = &command.receipt.envelope.agent_id;
    let row = store
        .agents
        .find_one(doc! {"_id":id.as_str()})
        .session(&mut *session)
        .await?
        .ok_or(CommandLedgerError::NotFound)?;
    let agent = super::agents::read_agent(&row).map_err(|_| CommandLedgerError::Corrupt)?;
    if agent.id != *id {
        return Err(CommandLedgerError::Corrupt.into());
    }
    super::ledger_maintenance_publication::fence_as(
        store,
        &agent,
        LedgerWriterClass::AgentWorker,
        session,
    )
    .await
    .map_err(|error| match error {
        TransactionLogError::Driver(error) => ApplyError::Driver(error),
        TransactionLogError::Domain(error) => ApplyError::Domain(match error {
            SandboxStoreError::Conflict => CommandLedgerError::NotAdmissible,
            SandboxStoreError::NotFound => CommandLedgerError::NotFound,
            SandboxStoreError::Corrupt(_) => CommandLedgerError::Corrupt,
            _ => CommandLedgerError::Unavailable,
        }),
    })
}
