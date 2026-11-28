use super::*;
use crate::store::agent_log_telemetry::ScanCaller;
use layer2_core::sandbox::{AgentLogEntry, AgentLogKind, FirstMessageDelivery};

/// Terminal refusal before dispatch still closes the preserved first-message
/// reservation. This emits evidence, never input or a synthetic turn.
pub(in crate::store) async fn project_terminal(
    store: &MongoSandboxStore,
    command: &AdmittedAgentCommand,
    session: &mut ClientSession,
) -> Result<(), SandboxStoreError> {
    if !command.status.is_terminal()
        || command.work.dispatch.is_some()
        || !command.receipt.envelope.is_first_message()
    {
        return Ok(());
    }
    let Some(row) = store
        .agents
        .find_one(doc! {"_id":command.receipt.envelope.agent_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(unavailable)?
    else {
        return Ok(());
    };
    let mut agent = super::super::agents::read_agent(&row)?;
    let binding = require_receipt(store, &agent, session)
        .await?
        .ok_or(SandboxStoreError::Conflict)?;
    binding
        .require_command(command)
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    let first = agent
        .conversation
        .first_message
        .as_mut()
        .ok_or(SandboxStoreError::Conflict)?;
    if first.state != FirstMessageDelivery::Queued {
        return Ok(());
    }
    store.fence_agent_effect(&agent.id, session).await?;
    let at = command
        .work
        .failure
        .as_ref()
        .map_or_else(layer2_core::now_rfc3339, |failure| failure.at.clone());
    first
        .fail_common_before_dispatch(at.clone())
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    let prior_agent = agent.clone();
    let previous_revision = agent.revision;
    let previous_seq = agent.conversation.last_seq;
    let mut entries = Vec::new();
    let summary = super::super::agent_log::scan_complete_log_with_session(
        ScanCaller::ConversationPublication,
        store,
        &agent.id,
        previous_seq,
        session,
        |entry| {
            entries.push(entry);
            Ok(())
        },
    )
    .await
    .map_err(|error| match error {
        super::super::agent_log_integrity::TransactionLogError::Domain(error) => error,
        super::super::agent_log_integrity::TransactionLogError::Driver(error) => unavailable(error),
    })?;
    agent.revision = previous_revision
        .checked_add(1)
        .ok_or(SandboxStoreError::Conflict)?;
    agent.conversation.last_seq = previous_seq
        .checked_add(1)
        .ok_or(SandboxStoreError::SequenceConflict)?;
    agent.updated_at = at.clone();
    let entry = AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: agent.conversation.last_seq,
        appended_at: at,
        kind: AgentLogKind::FirstMessageChanged {
            state: FirstMessageDelivery::Failed,
            recovery_generation: 0,
        },
    };

    let semantic =
        crate::store::agent_log_integrity::audit_checkpoint(store, &prior_agent, &entries, session)
            .await
            .map_err(|error| match error {
                super::super::agent_log_integrity::TransactionLogError::Domain(error) => error,
                super::super::agent_log_integrity::TransactionLogError::Driver(error) => {
                    unavailable(error)
                }
            })?;
    let integrity = super::super::agent_log_integrity::prepare_append(
        &agent,
        &summary,
        std::slice::from_ref(&entry),
        agent.revision,
        semantic.after_append(std::slice::from_ref(&entry), &agent)?,
        super::super::agent_log_integrity::AppendCapacity::Normal,
    )?;
    let mut fields = super::super::agents::agent_replacement_fields(&agent)?;
    fields.insert("agent_log_integrity_v1", integrity.integrity);
    fields.insert("last_seq", agent.conversation.last_seq as i64);
    let changed = store.agents.update_one(doc! {"_id":agent.id.as_str(),"revision":previous_revision as i64,"last_seq":previous_seq as i64},
        doc! {"$set":fields}).session(&mut *session).await.map_err(unavailable)?;
    if changed.matched_count != 1 {
        return Err(SandboxStoreError::Conflict);
    }
    store
        .agent_log
        .clone_with_type::<mongodb::bson::RawDocumentBuf>()
        .insert_one(&integrity.documents[0])
        .session(session)
        .await
        .map_err(unavailable)?;
    Ok(())
}
