use super::{agent_log::read_entry, sandbox_mongo::MongoSandboxStore};
use layer2_core::sandbox::{store::SandboxStoreError, Agent, AgentStatus, AgentTaskStatus};
use mongodb::{bson::doc, ClientSession};

pub(super) async fn read(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
) -> Result<AgentTaskStatus, SandboxStoreError> {
    if agent.status != AgentStatus::Ready {
        return Ok(AgentTaskStatus::from_latest(agent, None, false));
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
        pending.is_some(),
    ))
}
