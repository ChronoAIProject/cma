use super::*;
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
        || !matches!(
            command.receipt.envelope.actor,
            layer2_core::sandbox::agent_io::AgentActor::Trigger { .. }
        )
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
    let original = agent.clone();
    let first = agent
        .conversation
        .first_message
        .as_mut()
        .ok_or(SandboxStoreError::Conflict)?;
    if first.state != FirstMessageDelivery::Queued {
        return Ok(());
    }
    let at = command
        .work
        .failure
        .as_ref()
        .map_or_else(layer2_core::now_rfc3339, |failure| failure.at.clone());
    first
        .fail_common_before_dispatch(at.clone())
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    let previous_revision = agent.revision;
    let previous_seq = agent.conversation.last_seq;
    let summary = super::super::agent_log_context::validate(store, &original, session, false)
        .await
        .map_err(|error| match error {
            super::super::agent_log_integrity::TransactionLogError::Domain(error) => error,
            super::super::agent_log_integrity::TransactionLogError::Driver(error) => {
                unavailable(error)
            }
        })?
        .summary;
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
    super::super::agent_log_integrity::ensure_append_capacity(
        &summary,
        std::slice::from_ref(&entry),
        super::super::agent_log_integrity::AppendCapacity::Normal,
    )?;
    let mut fields = super::super::agents::agent_replacement_fields(&agent)?;
    fields.insert("last_seq", agent.conversation.last_seq as i64);
    let changed = store.agents.update_one(doc! {"_id":agent.id.as_str(),"revision":previous_revision as i64,"last_seq":previous_seq as i64},
        doc! {"$set":fields}).session(&mut *session).await.map_err(unavailable)?;
    if changed.matched_count != 1 {
        return Err(SandboxStoreError::Conflict);
    }
    store
        .agent_log
        .insert_one(super::super::agent_log::log_document(&entry)?)
        .session(session)
        .await
        .map_err(unavailable)?;
    Ok(())
}
