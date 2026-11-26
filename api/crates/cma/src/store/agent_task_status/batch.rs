//! Fetch bounded status evidence for a page in the caller's existing snapshot.
use std::collections::HashMap;

use layer2_core::AgentId;
use mongodb::bson::{Bson, Document};

use super::*;

const BATCH_SIZE: usize = 100;
const EVIDENCE_BYTES: i32 = 64 * 1024;

pub(in crate::store) struct TaskEvidence {
    pub observation: Observation,
    pub activity_at: Option<String>,
}

// Avoid combining several individually valid 16 MiB records into an invalid
// Mongo result. Large evidence keeps the original sequential read contract.
fn bounded() -> Document {
    let large = doc! {"$gt": [{"$bsonSize": "$$ROOT"}, EVIDENCE_BYTES]};
    doc! {"large": large.clone(), "document": {"$cond": [large, Bson::Null, "$$ROOT"]}}
}

pub(super) fn pipeline(agents: &[&Agent], include_activity: bool) -> Vec<Document> {
    let read_status = if include_activity {
        doc! {"$eq": ["$status", "ready"]}
    } else {
        doc! {"$literal": true}
    };
    let mut stages = vec![
        doc! {"$match": {"_id": {"$in": agents.iter().map(|a| a.id.as_str()).collect::<Vec<_>>()}}},
        doc! {"$project": {"_id": 1, "last_seq": 1, "read_status": read_status}},
        doc! {"$lookup": {
            "from": "agent_log", "localField": "_id", "foreignField": "agent_id", "as": "latest",
            "let": {"read_status": "$read_status"},
            "pipeline": [
                {"$match": {"$expr": "$$read_status", "record.kind.type": {"$in": ["created", "turn_started", "turn_completed", "turn_failed", "turn_interrupted", "conversation_gap"]}}},
                {"$sort": {"seq": -1}}, {"$limit": 1}, {"$replaceWith": bounded()},
            ],
        }},
        doc! {"$set": {"latest": {"$arrayElemAt": ["$latest", 0]}}},
        doc! {"$lookup": {
            "from": "agent_log", "localField": "_id", "foreignField": "agent_id", "as": "gap",
            "let": {"latest": "$latest.document", "head": "$last_seq"},
            "pipeline": [
                {"$match": {"record.kind.type": "conversation_gap", "$expr": {"$and": [
                    {"$eq": ["$$latest.record.kind.type", "turn_completed"]},
                    {"$lte": ["$$latest.seq", "$$head"]},
                    {"$gt": ["$seq", "$$latest.record.kind.turn_sequence"]},
                    {"$lt": ["$seq", "$$latest.seq"]},
                ]}}},
                {"$sort": {"seq": -1}}, {"$limit": 1}, {"$replaceWith": bounded()},
            ],
        }},
        doc! {"$lookup": {
            "from": "agent_commands", "localField": "_id", "foreignField": "agent_id", "as": "pending",
            "let": {"read_status": "$read_status"},
            "pipeline": [
                {"$match": {"$expr": "$$read_status", "status": {"$in": ["queued", "in_progress", "waiting_action"]}}},
                {"$limit": 1}, {"$replaceWith": bounded()},
            ],
        }},
        doc! {"$project": {
            "_id": 1, "last_seq": 1, "latest": 1,
            "gap": {"$arrayElemAt": ["$gap", 0]},
            "pending": {"$arrayElemAt": ["$pending", 0]},
        }},
    ];
    // Activity uses a head-fenced gap and start. Status uses the gap between
    // the observed start and completion, and also detects a row past the head.
    if include_activity {
        stages.extend(activity::lookups());
    }
    stages
}

pub(in crate::store) async fn read_many<'a>(
    store: &MongoSandboxStore,
    agents: impl IntoIterator<Item = &'a Agent>,
    session: &mut ClientSession,
) -> Result<HashMap<AgentId, AgentTaskStatus>, SandboxStoreError> {
    Ok(read_batch(store, agents, session, false)
        .await?
        .into_iter()
        .map(|(id, evidence)| (id, evidence.observation.0))
        .collect())
}

pub(in crate::store) async fn read_workspace(
    store: &MongoSandboxStore,
    agents: &[Agent],
    session: &mut ClientSession,
) -> Result<HashMap<AgentId, TaskEvidence>, SandboxStoreError> {
    read_batch(store, agents, session, true).await
}

async fn read_batch<'a>(
    store: &MongoSandboxStore,
    agents: impl IntoIterator<Item = &'a Agent>,
    session: &mut ClientSession,
    include_activity: bool,
) -> Result<HashMap<AgentId, TaskEvidence>, SandboxStoreError> {
    let mut results = HashMap::new();
    let mut queried = Vec::new();
    for agent in agents {
        if include_activity || agent.status == AgentStatus::Ready {
            queried.push(agent);
        } else {
            results.insert(
                agent.id.clone(),
                TaskEvidence {
                    observation: (AgentTaskStatus::from_latest(agent, None, None, false), None),
                    activity_at: None,
                },
            );
        }
    }
    for batch in queried.chunks(BATCH_SIZE) {
        let mut cursor = store
            .agents
            .aggregate(pipeline(batch, include_activity))
            .max_time(std::time::Duration::from_secs(10))
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
        let mut remaining: HashMap<_, _> = batch
            .iter()
            .map(|agent| (agent.id.as_str(), *agent))
            .collect();
        while cursor.advance(&mut *session).await.map_err(unavailable)? {
            let row: Document = cursor.deserialize_current().map_err(unavailable)?;
            let id = row.get_str("_id").map_err(|_| corrupt())?;
            let agent = remaining.remove(id).ok_or_else(corrupt)?;
            let activity_at = if include_activity {
                activity::read(agent, &row)?
            } else {
                None
            };
            let observation = match decode(agent, &row)? {
                Some(status) => status,
                None => super::read_with_turn(store, agent, session).await?,
            };
            results.insert(
                agent.id.clone(),
                TaskEvidence {
                    observation,
                    activity_at,
                },
            );
        }
        if !remaining.is_empty() {
            return Err(corrupt());
        }
    }
    Ok(results)
}

fn decode(agent: &Agent, row: &Document) -> Result<Option<Observation>, SandboxStoreError> {
    if agent.status != AgentStatus::Ready {
        return Ok(Some((
            AgentTaskStatus::from_latest(agent, None, None, false),
            None,
        )));
    }
    for field in ["latest", "gap", "pending"] {
        if let Some(value) = row.get(field) {
            if value
                .as_document()
                .ok_or_else(corrupt)?
                .get_bool("large")
                .map_err(|_| corrupt())?
            {
                return Ok(None);
            }
        }
    }
    let latest = evidence(row, "latest")?.map(read_entry).transpose()?;
    let gap = evidence(row, "gap")?.map(read_entry).transpose()?;
    let pending = evidence(row, "pending")?;
    if let Some(document) = pending {
        crate::store::agent_commands::read_command(document).map_err(|_| corrupt())?;
    }
    Ok(Some((
        AgentTaskStatus::from_latest(agent, latest.as_ref(), gap.as_ref(), pending.is_some()),
        AgentTaskStatus::observed_turn(agent, latest.as_ref()),
    )))
}

fn evidence<'a>(row: &'a Document, field: &str) -> Result<Option<&'a Document>, SandboxStoreError> {
    row.get(field)
        .map(|value| {
            value
                .as_document()
                .ok_or_else(corrupt)?
                .get_document("document")
                .map_err(|_| corrupt())
        })
        .transpose()
}

fn corrupt() -> SandboxStoreError {
    SandboxStoreError::Corrupt("task status batch evidence is invalid".into())
}

fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

#[cfg(test)]
mod tests;
