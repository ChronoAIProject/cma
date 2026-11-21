//! Fetch fresh provenance in one round trip under the caller's transaction.

use layer2_core::resource_access::{
    ResourceAccessSnapshot, ResourceAccessStoreError, ResourceLineage, ResourceTarget,
    MAX_ACTIVE_RESOURCE_SHARES,
};
use layer2_core::sandbox::store::AgentProvenanceSnapshot;
use layer2_core::AgentId;
use mongodb::bson::{doc, Bson, Document};
use mongodb::ClientSession;

use super::super::{
    agent_log::read_entry,
    agents::read_agent,
    resource_access::{records, ReadError},
    sandbox_mongo::{
        MongoSandboxStore, AGENT_LOG, RESOURCE_POLICIES, RESOURCE_SHARES, SANDBOXES, WORKSPACES,
        WORKTREES,
    },
    sandboxes::read_sandbox,
    workspaces::read_workspace,
    worktrees::read_worktree,
};

// Even 129 shares and two rows per singular lookup stay below 9 MiB, leaving
// room below Mongo's 16 MiB result limit. Oversized documents are represented
// by a marker, then reread sequentially in this very same snapshot.
const BATCH_DOCUMENT_BYTES: i32 = 64 * 1024;
const JOINED_FIELDS: [&str; 7] = [
    "worktree",
    "workspace",
    "sandbox",
    "policy",
    "shares",
    "creation",
    "first_log",
];

fn bounded_document() -> Document {
    let oversized = doc! {"$gt": [{"$bsonSize": "$$ROOT"}, BATCH_DOCUMENT_BYTES]};
    doc! {
        "oversized": oversized.clone(),
        "document": {"$cond": [oversized, Bson::Null, "$$ROOT"]},
    }
}

fn lookup(collection: &str, local: &str, name: &str) -> Document {
    doc! {"$lookup": {
        "from": collection, "localField": local, "foreignField": "_id", "as": name,
        "pipeline": [{"$limit": 2}, {"$replaceWith": bounded_document()}],
    }}
}

fn pipeline(id: &AgentId) -> Vec<Document> {
    vec![
        doc! {"$match": {"_id": id.as_str()}},
        doc! {"$replaceWith": {"agent": bounded_document()}},
        // All stored parent references must agree with the decoded lineage.
        // Missing or mismatched parents are rejected by the original validators.
        lookup(WORKTREES, "agent.document.record.worktree_id", "worktree"),
        lookup(
            WORKSPACES,
            "agent.document.record.workspace_id",
            "workspace",
        ),
        lookup(SANDBOXES, "agent.document.record.sandbox_id", "sandbox"),
        lookup(
            RESOURCE_POLICIES,
            "agent.document.record.sandbox_id",
            "policy",
        ),
        doc! {"$lookup": {
            "from": RESOURCE_SHARES,
            "localField": "agent.document.record.sandbox_id", "foreignField": "sandbox_id",
            "let": {"sandbox_id": "$agent.document.record.sandbox_id"}, "as": "shares",
            "pipeline": [
                {"$match": {"revoked": false, "$or": [
                    {"selector_kind": "agent", "selector_id": id.as_str()},
                    // Match the original equality query's array semantics so
                    // malformed projected grants reach read_share and fail.
                    {"selector_kind": "sandbox", "$expr": {"$cond": [
                        {"$isArray": "$selector_id"},
                        {"$in": ["$$sandbox_id", "$selector_id"]},
                        {"$eq": ["$selector_id", "$$sandbox_id"]},
                    ]}},
                ]}},
                {"$sort": {"_id": 1}},
                {"$limit": MAX_ACTIVE_RESOURCE_SHARES as i64 + 1},
                {"$replaceWith": bounded_document()},
            ],
        }},
        doc! {"$lookup": {
            "from": "agent_creation_provenance", "as": "creation",
            "pipeline": [
                {"$match": {"_id": id.as_str()}}, {"$limit": 2},
                {"$replaceWith": bounded_document()},
            ],
        }},
        doc! {"$lookup": {
            "from": AGENT_LOG, "as": "first_log",
            "pipeline": [
                {"$match": {"agent_id": id.as_str(), "seq": 1_i64}}, {"$limit": 2},
                {"$replaceWith": bounded_document()},
            ],
        }},
    ]
}

pub(super) async fn read(
    store: &MongoSandboxStore,
    id: &AgentId,
    session: &mut ClientSession,
) -> Result<Option<AgentProvenanceSnapshot>, ReadError> {
    let mut cursor = store
        .agents
        .aggregate(pipeline(id))
        .session(&mut *session)
        .await?;
    if !cursor.advance(&mut *session).await? {
        return Err(ResourceAccessStoreError::NotFound.into());
    }
    let row = cursor.deserialize_current()?;
    decode(id, &row).map_err(Into::into)
}

fn decode(
    id: &AgentId,
    row: &Document,
) -> Result<Option<AgentProvenanceSnapshot>, ResourceAccessStoreError> {
    let corrupt = || ResourceAccessStoreError::Corrupt;
    let agent = row.get_document("agent").map_err(|_| corrupt())?;
    if agent.get_bool("oversized").map_err(|_| corrupt())? {
        return Ok(None);
    }
    for field in JOINED_FIELDS {
        for value in row.get_array(field).map_err(|_| corrupt())? {
            if value
                .as_document()
                .ok_or_else(corrupt)?
                .get_bool("oversized")
                .map_err(|_| corrupt())?
            {
                return Ok(None);
            }
        }
    }
    let agent = read_agent(agent.get_document("document").map_err(|_| corrupt())?)
        .map_err(|_| corrupt())?;
    let worktree =
        read_worktree(single(row, "worktree")?.ok_or_else(corrupt)?).map_err(|_| corrupt())?;
    let workspace =
        read_workspace(single(row, "workspace")?.ok_or_else(corrupt)?).map_err(|_| corrupt())?;
    let sandbox =
        read_sandbox(single(row, "sandbox")?.ok_or_else(corrupt)?).map_err(|_| corrupt())?;
    let lineage = ResourceLineage {
        sandbox,
        workspace: Some(workspace),
        worktree: Some(worktree),
        agent: Some(agent),
    };
    let target = ResourceTarget::Agent {
        agent_id: id.clone(),
    };
    lineage.validate(&target)?;
    let policy = single(row, "policy")?
        .map(records::read_policy)
        .transpose()?;
    let mut shares = Vec::new();
    for value in row.get_array("shares").map_err(|_| corrupt())? {
        shares.push(records::read_share(&lineage.sandbox.id, document(value)?)?);
        if shares.len() > MAX_ACTIVE_RESOURCE_SHARES {
            return Err(ResourceAccessStoreError::Capacity);
        }
    }
    let resource = ResourceAccessSnapshot {
        lineage,
        policy,
        shares,
    };
    resource.scope(&target)?;
    let creation = single(row, "creation")?
        .map(super::read)
        .transpose()
        .map_err(|_| corrupt())?;
    let first_log = single(row, "first_log")?
        .map(read_entry)
        .transpose()
        .map_err(|_| corrupt())?;
    Ok(Some(AgentProvenanceSnapshot {
        resource,
        creation,
        first_log,
    }))
}

fn document(value: &Bson) -> Result<&Document, ResourceAccessStoreError> {
    value
        .as_document()
        .and_then(|wrapper| wrapper.get_document("document").ok())
        .ok_or(ResourceAccessStoreError::Corrupt)
}

fn single<'a>(
    row: &'a Document,
    field: &str,
) -> Result<Option<&'a Document>, ResourceAccessStoreError> {
    match row
        .get_array(field)
        .map_err(|_| ResourceAccessStoreError::Corrupt)?
        .as_slice()
    {
        [] => Ok(None),
        [value] => document(value).map(Some),
        _ => Err(ResourceAccessStoreError::Corrupt),
    }
}
