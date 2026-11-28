use super::{
    agent_log::read_entry,
    sandbox_mongo::{signed, MongoSandboxStore},
};
use layer2_core::sandbox::{
    store::SandboxStoreError, Agent, AgentLogKind, AgentStatus, AgentTaskStatus,
};
use mongodb::{bson::doc, ClientSession};

mod activity;
mod batch;
pub(super) use batch::read_many;
pub(super) use batch::read_workspace;

type Observation = (AgentTaskStatus, Option<u64>);

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

async fn read_with_turn(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
) -> Result<Observation, SandboxStoreError> {
    if agent.status != AgentStatus::Ready {
        return Ok((AgentTaskStatus::from_latest(agent, None, None, false), None));
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
    Ok((
        AgentTaskStatus::from_latest(agent, latest.as_ref(), gap.as_ref(), pending.is_some()),
        AgentTaskStatus::observed_turn(agent, latest.as_ref()),
    ))
}

pub(super) async fn active_starts(
    store: &MongoSandboxStore,
    agents: &[Agent],
    session: &mut ClientSession,
) -> Result<std::collections::HashMap<layer2_core::AgentId, String>, SandboxStoreError> {
    let mut starts = std::collections::HashMap::new();
    for agent in agents {
        let Some(turn) = agent.conversation.active_turn else {
            continue;
        };
        // agent_log_task_status: equality on Agent/type, then reverse seq scan.
        // find_one limits the matching result to one row within the snapshot head.
        let document = store
            .agent_log
            .find_one(active_start_filter(
                &agent.id,
                turn,
                agent.conversation.last_seq,
            )?)
            .sort(doc! { "seq": -1 })
            .hint(mongodb::options::Hint::Name("agent_log_task_status".into()))
            .max_time(std::time::Duration::from_secs(10))
            .session(&mut *session)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        if let Some(document) = document {
            let entry = read_entry(&document)?;
            if entry.agent_id == agent.id
                && entry.seq <= agent.conversation.last_seq
                && matches!(entry.kind, AgentLogKind::TurnStarted { turn_sequence, .. } if turn_sequence == turn)
            {
                starts.insert(entry.agent_id, entry.appended_at);
            }
        }
    }
    Ok(starts)
}

fn active_start_filter(
    id: &layer2_core::AgentId,
    turn: u64,
    head: u64,
) -> Result<mongodb::bson::Document, SandboxStoreError> {
    Ok(
        doc! { "agent_id": id.as_str(), "record.kind.type": "turn_started",
        "record.kind.turn_sequence": signed(turn, "turn sequence").map_err(SandboxStoreError::Corrupt)?,
        "seq": { "$lte": signed(head, "last sequence").map_err(SandboxStoreError::Corrupt)? } },
    )
}

#[cfg(test)]
mod start_tests {
    use super::*;
    #[test]
    fn current_turn_lookup_cannot_cross_agent_or_confirmed_history_boundaries() {
        let query = active_start_filter(&layer2_core::AgentId("agent-a".into()), 7, 23).unwrap();
        assert_eq!(query.get_str("agent_id").unwrap(), "agent-a");
        assert_eq!(query.get_str("record.kind.type").unwrap(), "turn_started");
        assert_eq!(query.get_i64("record.kind.turn_sequence").unwrap(), 7);
        assert_eq!(
            query.get_document("seq").unwrap().get_i64("$lte").unwrap(),
            23
        );
        assert!(
            active_start_filter(&layer2_core::AgentId("agent-a".into()), u64::MAX, 23).is_err()
        );
    }
}
