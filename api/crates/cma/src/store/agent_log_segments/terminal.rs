//! Terminal Trigger first-message evidence in the original command transaction.
use super::{
    publication::{self, PublicationIdentity},
    read_format,
    transaction_scope::TransactionScope,
    writer_commit::{self, AggregateUpdate, PublicationInput, Recorded},
};
use crate::store::{
    agent_log_integrity::{AppendCapacity, TransactionLogError},
    agents::{agent_replacement_fields, read_agent},
    sandbox_mongo::MongoSandboxStore,
    trigger_first_messages,
};
use layer2_core::sandbox::{
    agent_io::{AdmittedAgentCommand, CommandLedgerError},
    store::SandboxStoreError,
    AgentLogEntry, AgentLogKind, FirstMessageDelivery,
};
use mongodb::{bson::doc, options::ReadConcern};

#[derive(Clone)]
pub(in crate::store) struct Receipt {
    recorded: Recorded,
    command: AdmittedAgentCommand,
    pub(super) related: Option<mongodb::bson::Document>,
}

pub(in crate::store) async fn project(
    store: &MongoSandboxStore,
    command: &AdmittedAgentCommand,
    at: &str,
    scope: &mut TransactionScope<'_>,
) -> Result<Option<Receipt>, TransactionLogError> {
    if !command.status.is_terminal()
        || command.work.dispatch.is_some()
        || !command.receipt.envelope.is_first_message()
    {
        return Ok(None);
    }
    let Some(row) = scope
        .find_one(
            &store.agents,
            doc! {"_id":command.receipt.envelope.agent_id.as_str()},
        )
        .await
        .map_err(TransactionLogError::Driver)?
    else {
        return Ok(None);
    };
    if read_format(&row)?.is_none() {
        trigger_first_messages::project_terminal(store, command, scope.session()).await?;
        return Ok(None);
    }
    let current = read_agent(&row)?;
    let binding = trigger_first_messages::require_receipt(store, &current, scope.session())
        .await?
        .ok_or(SandboxStoreError::Conflict)?;
    binding
        .require_command(command)
        .map_err(|e| SandboxStoreError::Corrupt(e.to_string()))?;
    let first = current
        .conversation
        .first_message
        .as_ref()
        .ok_or(SandboxStoreError::Conflict)?;
    if first.state != FirstMessageDelivery::Queued {
        return Ok(None);
    }
    store
        .fence_agent_effect(&current.id, scope.session())
        .await?;
    let mut next = current.clone();
    next.conversation
        .first_message
        .as_mut()
        .ok_or(SandboxStoreError::Conflict)?
        .fail_common_before_dispatch(at.into())
        .map_err(|e| SandboxStoreError::Corrupt(e.to_string()))?;
    next.revision = current
        .revision
        .checked_add(1)
        .ok_or(SandboxStoreError::Conflict)?;
    next.conversation.last_seq = current
        .conversation
        .last_seq
        .checked_add(1)
        .ok_or(SandboxStoreError::SequenceConflict)?;
    next.updated_at = at.into();
    let entry = AgentLogEntry {
        agent_id: current.id.clone(),
        seq: next.conversation.last_seq,
        appended_at: at.into(),
        kind: AgentLogKind::FirstMessageChanged {
            state: FirstMessageDelivery::Failed,
            recovery_generation: 0,
        },
    };
    let operation = PublicationIdentity {
        key: format!(
            "trigger-terminal:{}:{}",
            command.receipt.envelope.command_id, command.revision
        ),
        fingerprint: publication::request_digest(command)?,
        appended_at: at.into(),
    };
    let recorded = Box::pin(writer_commit::publish_existing(
        store,
        scope,
        PublicationInput {
            operation: &operation,
            current: &current,
            next: &next,
            logs: std::slice::from_ref(&entry),
            capacity: AppendCapacity::Normal,
            result_digest: publication::result_digest(&(&next, &entry, command))?,
        },
        AggregateUpdate {
            filter: doc! {"_id":current.id.as_str(),"channel_history_seal":{"$exists":false}},
            fields: agent_replacement_fields(&next)?,
        },
    ))
    .await?;
    Ok(Some(Receipt {
        recorded,
        command: command.clone(),
        related: None,
    }))
}

/// Absence remains unknown. This only reads the original receipt and immutable
/// terminal command in one snapshot, including after unrelated Agent appends.
pub(in crate::store) async fn reconcile(
    store: &MongoSandboxStore,
    receipt: Option<&Receipt>,
    at: Option<mongodb::bson::Timestamp>,
) -> Result<bool, CommandLedgerError> {
    let (Some(receipt), Some(at)) = (receipt, at) else {
        return Ok(false);
    };
    let store = store.clone();
    let receipt = receipt.clone();
    crate::store::sandbox_mongo::wait_for_reconciliation("segmented Trigger terminal", async move {
        Box::pin(reconcile_at(&store, &receipt, at)).await
    })
    .await
    .map_err(|_| CommandLedgerError::Unavailable)?
}

async fn reconcile_at(
    store: &MongoSandboxStore,
    receipt: &Receipt,
    at: mongodb::bson::Timestamp,
) -> Result<bool, CommandLedgerError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(at)
        .await
        .map_err(|_| CommandLedgerError::Unavailable)?;
    if !Box::pin(receipt.recorded.matches(store, &mut session))
        .await
        .map_err(|_| CommandLedgerError::Unavailable)?
    {
        return Ok(false);
    }
    if let Some(expected) = &receipt.related {
        let id = expected
            .get_str("_id")
            .map_err(|_| CommandLedgerError::Corrupt)?;
        let actual = store
            .agent_commands
            .find_one(doc! {"_id":id})
            .read_concern(ReadConcern::snapshot())
            .session(&mut session)
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
        if actual.as_ref() != Some(expected) {
            return Ok(false);
        }
    }
    let actual = store
        .agent_commands
        .find_one(doc! {"_id":receipt.command.receipt.envelope.command_id.as_str()})
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(|_| CommandLedgerError::Unavailable)?;
    let Some(actual) = actual else {
        return Ok(false);
    };
    let actual = crate::store::agent_commands::read_command(&actual)?;
    if actual != receipt.command {
        return Err(CommandLedgerError::Corrupt);
    }
    trigger_first_messages::require_command_receipt_snapshot(
        store,
        &receipt.recorded.next,
        &mut session,
    )
    .await
    .map_err(|_| CommandLedgerError::Corrupt)?;
    Ok(true)
}
