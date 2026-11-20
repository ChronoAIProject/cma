use std::collections::HashMap;

use layer2_core::{
    sandbox::task_activity::{TaskActivityEvidence, TASK_ACTIVITY_TYPES},
    AgentId,
};
use mongodb::bson::{Bson, Document};

use super::*;
use crate::store::{
    agent_log::log_id,
    sandbox_mongo::{i64_field_matches, read_record},
};

const BATCH_SIZE: usize = 100;

fn projection() -> Document {
    doc! {"$project": {"_id": 1, "document_schema_version": 1, "agent_id": 1, "seq": 1,
    "record.agent_id": 1, "record.seq": 1, "record.appended_at": 1,
    "record.kind.type": 1, "record.kind.turn_sequence": 1}}
}

fn lookup(types: &[&str], name: &str) -> Document {
    doc! {"$lookup": {
        "from": "agent_log", "localField": "_id", "foreignField": "agent_id", "as": name,
        "let": {"head": "$last_seq"},
        "pipeline": [
            {"$match": {"record.kind.type": {"$in": types}, "$expr": {"$lte": ["$seq", "$$head"]}}},
            {"$sort": {"seq": -1}}, {"$limit": 1}, projection(),
            // Even malformed timestamp/identity fields cannot inflate a batch response.
            {"$replaceWith": {"$cond": [{"$lte": [{"$bsonSize": "$$ROOT"}, 4096]}, "$$ROOT", {"invalid": true}]}},
        ],
    }}
}

fn pipeline(agents: &[Agent]) -> Vec<Document> {
    vec![
        doc! {"$match": {"_id": {"$in": agents.iter().map(|agent| agent.id.as_str()).collect::<Vec<_>>()}}},
        doc! {"$project": {"_id": 1, "last_seq": 1}},
        lookup(TASK_ACTIVITY_TYPES, "activity"),
        lookup(&["conversation_gap"], "gap"),
        lookup(&["turn_started"], "start"),
    ]
}

pub(in crate::store) async fn read_many(
    store: &MongoSandboxStore,
    agents: &[Agent],
    session: &mut ClientSession,
) -> Result<HashMap<AgentId, String>, SandboxStoreError> {
    let mut activity = HashMap::new();
    for batch in agents.chunks(BATCH_SIZE) {
        let mut remaining: HashMap<_, _> = batch
            .iter()
            .map(|agent| (agent.id.as_str(), agent))
            .collect();
        let mut cursor = store
            .agents
            .aggregate(pipeline(batch))
            .max_time(std::time::Duration::from_secs(10))
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
        while cursor.advance(&mut *session).await.map_err(unavailable)? {
            let row: Document = cursor.deserialize_current().map_err(unavailable)?;
            let id = row.get_str("_id").map_err(|_| corrupt())?;
            let agent = remaining.remove(id).ok_or_else(corrupt)?;
            if !i64_field_matches(
                &row,
                "last_seq",
                signed(agent.conversation.last_seq, "last sequence")
                    .map_err(SandboxStoreError::Corrupt)?,
            ) {
                continue;
            }
            if let Some(at) = decode(agent, &row) {
                activity.insert(agent.id.clone(), at);
            }
        }
        if !remaining.is_empty() {
            return Err(corrupt());
        }
    }
    Ok(activity)
}

fn evidence(row: &Document, field: &str) -> Option<Option<TaskActivityEvidence>> {
    let values = row.get_array(field).ok()?;
    match values.as_slice() {
        [] => Some(None),
        [Bson::Document(document)] => {
            let entry: TaskActivityEvidence = read_record(document).ok()?;
            if document.get_str("agent_id").ok()? != entry.agent_id.as_str()
                || document.get_str("_id").ok()? != log_id(&entry.agent_id, entry.seq)
                || !i64_field_matches(document, "seq", signed(entry.seq, "sequence").ok()?)
            {
                return None;
            }
            Some(Some(entry))
        }
        _ => None,
    }
}

fn decode(agent: &Agent, row: &Document) -> Option<String> {
    let entry = evidence(row, "activity")??;
    let gap = evidence(row, "gap")?;
    let start = evidence(row, "start")?;
    entry
        .timestamp(agent, start.as_ref(), gap.as_ref())
        .map(str::to_owned)
}

fn corrupt() -> SandboxStoreError {
    SandboxStoreError::Corrupt("task activity batch identity is invalid".into())
}
fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

#[cfg(test)]
mod tests;
