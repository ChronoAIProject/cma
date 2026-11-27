//! Validate and append the exact ordered lifecycle log batch.
use super::*;

pub(super) fn final_agent_and_logs(
    current: &Agent,
    mutation: &AgentWorkMutation,
) -> Result<(Agent, Vec<AgentLogEntry>), ApplyError> {
    let final_agent = mutation.agent.clone();
    final_agent
        .validate_conversation()
        .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
    let logs = mutation
        .agent_logs
        .iter()
        .enumerate()
        .map(|(offset, kind)| AgentLogEntry {
            agent_id: final_agent.id.clone(),
            seq: current.conversation.last_seq + offset as u64 + 1,
            appended_at: mutation.appended_at.clone(),
            kind: kind.clone(),
        })
        .collect();
    Ok((final_agent, logs))
}

pub(super) async fn insert_logs(
    store: &MongoSandboxStore,
    logs: &[mongodb::bson::RawDocumentBuf],
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    for entry in logs {
        store
            .agent_log
            .clone_with_type::<mongodb::bson::RawDocumentBuf>()
            .insert_one(entry)
            .session(&mut *session)
            .await
            .map_err(|error| {
                if is_duplicate_key(&error) {
                    ApplyError::Domain(SandboxStoreError::SequenceConflict)
                } else {
                    ApplyError::Driver(error)
                }
            })?;
    }
    Ok(())
}

pub(super) async fn write_recovery_activity(
    store: &MongoSandboxStore,
    agent: &Agent,
    update: Document,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    let result = store
        .agents
        .update_one(
            doc! {
                "_id": agent.id.as_str(),
                "revision": signed(agent.revision, "connection recovery revision")
                    .map_err(SandboxStoreError::Corrupt).map_err(ApplyError::Domain)?,
            },
            update,
        )
        .session(session)
        .await
        .map_err(ApplyError::Driver)?;
    if result.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    Ok(())
}
