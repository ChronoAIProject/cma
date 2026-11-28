use crate::store::agent_log_telemetry::ScanCaller;
use crate::telemetry::command_reconcile::Outcome;
use layer2_core::ids::AgentCommandId;
use layer2_core::sandbox::agent_io::{AdmittedAgentCommand, CommandEvidence, CommandLedgerError};
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::bson::doc;
use mongodb::ClientSession;

use super::super::agent_log::scan_complete_log_with_session;
use super::super::agent_log_integrity::TransactionLogError;
use super::super::agents::read_agent;
use super::super::sandbox_mongo::MongoSandboxStore;
use super::{lane_records, records, worker_apply, ApplyError};

#[cfg(test)]
pub(in crate::store) async fn apply(
    store: &MongoSandboxStore,
    id: &AgentCommandId,
    session: &mut ClientSession,
) -> Result<AdmittedAgentCommand, ApplyError> {
    apply_observed(store, id, session)
        .await
        .map(|(command, _)| command)
}

pub(super) async fn apply_observed(
    store: &MongoSandboxStore,
    id: &AgentCommandId,
    session: &mut ClientSession,
) -> Result<(AdmittedAgentCommand, Outcome), ApplyError> {
    let document = store
        .agent_commands
        .find_one(doc! { "_id": id.as_str() })
        .session(&mut *session)
        .await?
        .ok_or(CommandLedgerError::NotFound)?;
    let mut command = records::read(&document)?;
    if command.work.dispatch.is_none() || command.status.is_terminal() {
        return Ok((command, Outcome::Unchanged));
    }
    let agent_id = &command.receipt.envelope.agent_id;
    let agent = store
        .agents
        .find_one(doc! { "_id": agent_id.as_str() })
        .session(&mut *session)
        .await?
        .ok_or(CommandLedgerError::NotFound)?;
    let agent_row = agent;
    let agent = read_agent(&agent_row).map_err(|_| CommandLedgerError::Corrupt)?;
    let lane = store
        .agent_command_orders
        .find_one(doc! { "_id": agent_id.as_str() })
        .session(&mut *session)
        .await?
        .ok_or(CommandLedgerError::Corrupt)?;
    let mut lane = lane_records::read(&lane, agent_id.as_str())?;
    let evidence = match super::super::agent_log_segments::command_evidence::read_transaction(
        store, &agent, &agent_row, &command, session,
    )
    .await
    .map_err(evidence_error)?
    {
        Some(evidence) => evidence,
        // The segmented reader returns None exclusively for legacy roots.
        None => legacy_evidence(store, &agent, &agent_row, &command, session).await?,
    };
    let source_deferred = evidence.has_source_deferred();
    let retry_current = if source_deferred {
        // The ledger fold can run anywhere. Only this branch writes the Agent,
        // so tell the scheduler when the durable evidence needs an owner.
        if store.agent_ownership.is_some()
            && layer2_core::agent_ownership::execution_owner().is_none()
        {
            return Err(CommandLedgerError::StaleClaim.into());
        }
        if !worker_apply::write_parent_fence(store, &agent, session).await? {
            return Err(CommandLedgerError::Conflict.into());
        }
        store
            .fence_agent_effect(&agent.id, session)
            .await
            .map_err(|error| match error {
                SandboxStoreError::Unavailable(_) => CommandLedgerError::Unavailable,
                _ => CommandLedgerError::Conflict,
            })?;
        let current = store
            .bindings
            .continuation_retry_authority(&command.receipt.envelope, session)
            .await?;
        let fenced = store.agents.update_one(doc! {"_id":agent.id.as_str(),"revision":i64::try_from(agent.revision).map_err(|_|CommandLedgerError::Corrupt)?},doc! {"$inc":{"agent_command_admission_fence":1_i64}}).session(&mut *session).await?;
        if fenced.matched_count != 1 {
            return Err(CommandLedgerError::Conflict.into());
        }
        current
    } else {
        false
    };
    let revision = command.revision;
    let written =
        evidence.apply_with_continuation_retry(&mut command, &mut lane, &agent, retry_current)?;
    if written {
        if !source_deferred && !worker_apply::fence_parent(store, Some(&agent), session).await? {
            return Err(CommandLedgerError::Conflict.into());
        }
        worker_apply::write(store, &command, revision, &lane, session).await?;
    }
    let outcome = if !written {
        Outcome::Unchanged
    } else if command.status.is_terminal() {
        Outcome::Settled
    } else {
        Outcome::Advanced
    };
    Ok((command, outcome))
}

async fn legacy_evidence(
    store: &MongoSandboxStore,
    agent: &layer2_core::sandbox::Agent,
    agent_row: &mongodb::bson::Document,
    command: &AdmittedAgentCommand,
    session: &mut ClientSession,
) -> Result<CommandEvidence, ApplyError> {
    let agent_id = &agent.id;
    let mut evidence = CommandEvidence::new(command)?;
    let dispatch = command
        .work
        .dispatch
        .as_ref()
        .ok_or(CommandLedgerError::Corrupt)?;
    let suffix_length = agent
        .conversation
        .last_seq
        .checked_sub(dispatch.operation_sequence)
        .and_then(|value| value.checked_add(1));
    let bounded = matches!(
        command.receipt.envelope.command,
        layer2_core::sandbox::agent_io::AgentCommand::Message { .. }
    ) && !command.receipt.envelope.is_first_message()
        && dispatch.operation_sequence.checked_add(1) == Some(dispatch.turn_sequence)
        && suffix_length.is_some_and(|length| length <= 16)
        && super::super::agent_log_integrity::trusted_summary(
            store,
            agent,
            agent_row,
            ScanCaller::CommandReconciliation,
            session,
            false,
        )
        .await
        .map_err(evidence_error)?
        .is_some();
    if bounded {
        use futures_util::TryStreamExt;
        evidence = CommandEvidence::from_trusted_message_prefix(command)?;
        let mut observation =
            crate::store::mongo_read_telemetry::Read::find(store.agent_log.name());
        let collection = store
            .agent_log
            .clone_with_type::<mongodb::bson::RawDocumentBuf>();
        let mut rows = collection
            .find(doc! { "agent_id": agent_id.as_str(),
            "seq": { "$gte": dispatch.operation_sequence as i64 } })
            .sort(doc! { "seq": 1 })
            .limit(17)
            .session(&mut *session)
            .await?;
        while let Some(raw) = rows.stream(&mut *session).try_next().await? {
            observation.document(raw.as_bytes().len());
            let row =
                mongodb::bson::Document::try_from(&raw).map_err(|_| CommandLedgerError::Corrupt)?;
            evidence.observe(
                &super::super::agent_log::read_entry(&row)
                    .map_err(|_| CommandLedgerError::Corrupt)?,
            )?;
        }
        observation.complete();
    } else {
        scan_complete_log_with_session(
            ScanCaller::CommandReconciliation,
            store,
            agent_id,
            agent.conversation.last_seq,
            session,
            |entry| {
                evidence
                    .observe(&entry)
                    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
            },
        )
        .await
        .map_err(evidence_error)?;
    }
    Ok(evidence)
}

fn evidence_error(error: TransactionLogError) -> ApplyError {
    match error {
        TransactionLogError::Domain(SandboxStoreError::Unavailable(_)) => {
            ApplyError::Domain(CommandLedgerError::Unavailable)
        }
        TransactionLogError::Domain(_) => ApplyError::Domain(CommandLedgerError::Corrupt),
        TransactionLogError::Driver(error) => ApplyError::Driver(error),
    }
}
