use super::{
    agent_log::read_entry,
    sandbox_mongo::{signed, MongoSandboxStore},
};
use layer2_core::sandbox::{
    store::SandboxStoreError, Agent, AgentLogKind, AgentStatus, AgentTaskStatus,
};
use mongodb::{bson::doc, ClientSession};

mod batch;
pub(super) use batch::read_many;

pub(super) async fn ensure_indexes(
    collection: &mongodb::Collection<mongodb::bson::Document>,
) -> Result<(), layer2_core::StoreError> {
    collection
        .create_index(
            mongodb::IndexModel::builder()
                .keys(doc! {"agent_id": 1, "record.kind.type": 1, "seq": -1})
                .options(
                    mongodb::options::IndexOptions::builder()
                        .name("agent_log_task_status".to_owned())
                        .build(),
                )
                .build(),
        )
        .await
        .map(|_| ())
        .map_err(|error| {
            layer2_core::StoreError::Unavailable(format!("agent task status index: {error}"))
        })
}

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

pub(super) async fn active_starts(
    store: &MongoSandboxStore,
    agents: &[Agent],
    session: &mut ClientSession,
) -> Result<std::collections::HashMap<layer2_core::AgentId, String>, SandboxStoreError> {
    use futures_util::TryStreamExt as _;
    let active: std::collections::HashMap<_, _> = agents
        .iter()
        .filter_map(|agent| {
            agent
                .conversation
                .active_turn
                .map(|turn| (agent.id.clone(), (turn, agent.conversation.last_seq)))
        })
        .collect();
    if active.is_empty() {
        return Ok(Default::default());
    }
    let terms = active.iter().map(|(id, (turn, last))| Ok(doc! {
        "agent_id": id.as_str(), "record.kind.turn_sequence": signed(*turn, "turn sequence").map_err(SandboxStoreError::Corrupt)?,
        "seq": { "$lte": signed(*last, "last sequence").map_err(SandboxStoreError::Corrupt)? },
    })).collect::<Result<Vec<_>, SandboxStoreError>>()?;
    let mut cursor = store
        .agent_log
        .find(doc! { "record.kind.type": "turn_started", "$or": terms })
        .max_time(std::time::Duration::from_secs(10))
        .session(&mut *session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let mut starts = std::collections::HashMap::new();
    while let Some(document) = cursor
        .stream(&mut *session)
        .try_next()
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
    {
        let entry = read_entry(&document)?;
        if let Some((turn, last)) = active.get(&entry.agent_id) {
            if entry.seq <= *last
                && matches!(entry.kind, AgentLogKind::TurnStarted { turn_sequence, .. } if turn_sequence == *turn)
            {
                starts.insert(entry.agent_id, entry.appended_at);
            }
        }
    }
    Ok(starts)
}
