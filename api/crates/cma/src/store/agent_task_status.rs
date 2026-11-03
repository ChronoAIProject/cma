use super::{
    agent_log::read_entry,
    sandbox_mongo::{signed, MongoSandboxStore},
};
use layer2_core::sandbox::{
    store::SandboxStoreError, Agent, AgentLogKind, AgentStatus, AgentTaskStatus,
};
use mongodb::{bson::doc, ClientSession};

pub(super) async fn read(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
) -> Result<AgentTaskStatus, SandboxStoreError> {
    if agent.status != AgentStatus::Ready {
        return Ok(AgentTaskStatus::from_latest(agent, None, None, false));
    }
    let document = store.agent_log.find_one(doc! {
        "agent_id": agent.id.as_str(),
        "record.kind.type": { "$in": ["created", "turn_started", "turn_completed", "turn_failed", "turn_interrupted", "conversation_gap"] },
    })
        .sort(doc! { "seq": -1 })
        .max_time(std::time::Duration::from_secs(10))
        .session(&mut *session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let latest = document.as_ref().map(read_entry).transpose()?;
    let gap = if let Some(entry) = latest
        .as_ref()
        .filter(|entry| entry.seq <= agent.conversation.last_seq)
    {
        if let AgentLogKind::TurnCompleted { turn_sequence } = entry.kind {
            store.agent_log.find_one(doc! {
                "agent_id": agent.id.as_str(),
                "seq": {
                    "$gt": signed(turn_sequence, "turn sequence").map_err(SandboxStoreError::Corrupt)?,
                    "$lt": signed(entry.seq, "completion sequence").map_err(SandboxStoreError::Corrupt)?,
                },
                "record.kind.type": "conversation_gap",
            })
                .sort(doc! { "seq": -1 })
                .max_time(std::time::Duration::from_secs(10))
                .session(&mut *session)
                .await
                .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
                .as_ref().map(read_entry).transpose()?
        } else {
            None
        }
    } else {
        None
    };
    let pending = store.agent_commands.find_one(doc! {
        "agent_id": agent.id.as_str(), "status": { "$in": ["queued", "in_progress", "waiting_action"] },
    }).max_time(std::time::Duration::from_secs(10)).session(session).await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    if let Some(document) = &pending {
        super::agent_commands::read_command(document)
            .map_err(|_| SandboxStoreError::Corrupt("task command evidence is invalid".into()))?;
    }
    Ok(AgentTaskStatus::from_latest(
        agent,
        latest.as_ref(),
        gap.as_ref(),
        pending.is_some(),
    ))
}
