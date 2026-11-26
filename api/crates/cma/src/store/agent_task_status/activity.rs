use layer2_core::sandbox::task_activity::{TaskActivityEvidence, TASK_ACTIVITY_TYPES};
use layer2_core::sandbox::{store::SandboxStoreError, Agent};
use mongodb::bson::{Bson, Document};

use crate::store::{
    agent_log::log_id,
    sandbox_mongo::{i64_field_matches, read_record, signed},
};
use mongodb::bson::doc;

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

pub(super) fn lookups() -> [Document; 3] {
    [
        lookup(TASK_ACTIVITY_TYPES, "activity"),
        lookup(&["conversation_gap"], "activity_gap"),
        lookup(&["turn_started"], "activity_start"),
    ]
}

pub(super) fn read(agent: &Agent, row: &Document) -> Result<Option<String>, SandboxStoreError> {
    let head =
        signed(agent.conversation.last_seq, "last sequence").map_err(SandboxStoreError::Corrupt)?;
    if !i64_field_matches(row, "last_seq", head) {
        return Ok(None);
    }
    Ok(decode(agent, row))
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
    let gap = evidence(row, "activity_gap")?;
    let start = evidence(row, "activity_start")?;
    entry
        .timestamp(agent, start.as_ref(), gap.as_ref())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests;
