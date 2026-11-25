//! Durable disposition of a bound-runtime read after receiver cancellation.
use super::agent_ownership::FenceError;
use super::sandbox_mongo::{
    commit_transaction, is_transient, signed, start_transaction, CommitOutcome, MongoSandboxStore,
    TRANSACTION_ATTEMPTS,
};
use super::transaction_terminal::GuardedTerminal as _;
use layer2_core::agent_ownership::{bounded, OwnershipError, OwnershipFailure, OwnershipToken};
use layer2_core::sandbox::{store::SandboxStoreError, Agent, RuntimeBinding};
use layer2_core::AgentId;
use mongodb::bson::{doc, Bson, Document};
use mongodb::ClientSession;

mod claim;
mod confirm;
pub(super) use confirm::confirm;
pub(super) mod recovery;
pub(super) use claim::{for_claim, ClaimPark};
pub(crate) use recovery::command;

pub(super) const FIELD: &str = "runtime_inspection_park_v1";
pub(super) const CLAIM_PROOF: &str = "runtime_inspection_claim_v1";
const PARENT: &str = "__inspection_park_parent";

impl MongoSandboxStore {
    pub(crate) fn with_agent_inspection_parking(mut self, enabled: bool) -> Self {
        self.agent_inspection_parking = enabled;
        self
    }
}

pub(super) fn has_marker(row: &Document) -> bool {
    row.contains_key(FIELD)
}

pub(super) fn filter(store: &MongoSandboxStore, matching: bool) -> Vec<Document> {
    if !store.agent_inspection_parking {
        return vec![];
    }
    let marker = format!("${FIELD}");
    let same_parent = doc! {"$and": [
        {"$eq": ["$owner", "$$owner"]},
        {"$eq": ["$record.runtime", "$$binding"]},
    ]};
    let active = doc! {"$and": [
        {"$eq": [format!("{marker}.version"), 1_i64]},
        {"$eq": [{"$type": format!("{marker}.id")}, "string"]},
        {"$eq": [format!("{marker}.step"), "$record.work.step"]},
        {"$eq": [format!("{marker}.fence"), "$record.work.next_fence"]},
        {"$eq": [format!("{marker}.ready_at"), "$record.work.ready_at"]},
        {"$eq": [format!("{marker}.attempts"), "$record.work.attempts"]},
        {"$ne": [format!("{marker}.retry"), true]},
        {"$ne": [format!("{marker}.inspection_confirmed"), true]},
        {"$or": [
            {"$ne": [{"$type": format!("{marker}.probe_at")}, "date"]},
            {"$gt": [format!("{marker}.probe_at"), "$$NOW"]},
        ]},
    ]};
    let parked = doc! {"$gt": [{"$size": format!("${PARENT}")}, 0]};
    vec![
        doc! {"$lookup": {
            "from": store.sandboxes.name(), "localField": "record.sandbox_id", "foreignField": "_id",
            "let": {"active": active, "owner": "$owner", "binding": format!("{marker}.binding")},
            "pipeline": [
                {"$match": {"$expr": {"$and": ["$$active", same_parent]}}},
                {"$project": {"_id": 1}}
            ],
            "as": PARENT,
        }},
        doc! {"$match": {"$expr": if matching { parked } else {doc! {"$not": [parked]}}}},
        doc! {"$unset": PARENT},
    ]
}

pub(super) async fn parked(
    store: &MongoSandboxStore,
    id: &AgentId,
) -> Result<bool, SandboxStoreError> {
    if !store.agent_inspection_parking {
        return Ok(false);
    }
    let mut query = vec![doc! {"$match": {"_id": id.as_str(), FIELD: {"$exists": true}}}];
    query.extend(filter(store, true));
    query.push(doc! {"$project": {"_id": 1}});
    let mut cursor = store.agents.aggregate(query).await.map_err(unavailable)?;
    cursor.advance().await.map_err(unavailable)
}

pub(super) async fn park(
    store: &MongoSandboxStore,
    owner: &OwnershipToken,
    claimed: &Agent,
    binding: &RuntimeBinding,
    failure: OwnershipFailure,
) -> Result<bool, SandboxStoreError> {
    if !store.agent_inspection_parking || owner.agent_id != claimed.id {
        return Ok(false);
    }
    let Some(ownership) = &store.agent_ownership else {
        return Ok(false);
    };
    let Some(lease) = &claimed.work.lease else {
        return Ok(false);
    };
    let signed = |value, field| signed(value, field).map_err(SandboxStoreError::Corrupt);
    let revision = signed(claimed.revision, "Agent revision")?;
    let epoch = signed(owner.epoch, "owner epoch")?;
    let id = uuid::Uuid::new_v4().to_string();
    let agent_filter = doc! {
        "_id": claimed.id.as_str(), "owner": claimed.owner.as_str(), "revision": revision,
        "record.revision": revision, "record.sandbox_id": claimed.sandbox_id.as_str(),
        "record.work.step": claimed.work.step.operation_name(), "agent_manager_work_epoch": epoch,
        "work_lease_holder": &lease.holder, "work_lease_fence": signed(lease.fence, "work fence")?,
        "work_lease_expires_at": &lease.expires_at,
        "$expr": {"$gt": ["$work_lease_expires_at", {"$dateToString": {"date": "$$NOW", "format": "%Y-%m-%dT%H:%M:%SZ"}}]},
    };
    let marker = doc! {
        "version": 1_i64, "id": &id, "step": claimed.work.step.operation_name(),
        "fence": signed(claimed.work.next_fence, "next work fence")?, "epoch": epoch,
        "ready_at": &claimed.work.ready_at, "attempts": i64::from(claimed.work.attempts),
        "phase": failure.phase.as_str(), "reason": failure.reason.as_str(),
        "http_status": failure.http_status.map(i32::from),
    };
    let result = bounded(async {
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = store
                .client
                .start_session()
                .await
                .map_err(ownership_unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(ownership_unavailable)?;
            let settlement = async {
                ownership
                    .fence_transaction_typed(owner, &mut session)
                    .await?;
                block_work(
                    store,
                    claimed,
                    binding,
                    &agent_filter,
                    &marker,
                    &mut session,
                )
                .await
            }
            .await;
            match settlement {
                Ok(true) => match commit_transaction(&mut session).await {
                    CommitOutcome::Committed => return Ok(true),
                    CommitOutcome::Retry(_) => continue,
                    CommitOutcome::Unknown(_) => {
                        return store
                            .agents
                            .find_one(doc! {"_id": claimed.id.as_str(),
                            format!("{FIELD}.id"): &id})
                            .projection(doc! {"_id": 1})
                            .await
                            .map(|row| row.is_some())
                            .map_err(ownership_unavailable);
                    }
                    CommitOutcome::Failed(error) => return Err(ownership_unavailable(error)),
                },
                outcome => {
                    session
                        .abort_guarded()
                        .await
                        .map_err(ownership_unavailable)?;
                    match outcome {
                        Ok(false) | Err(FenceError::Domain(SandboxStoreError::Conflict)) => {
                            return Ok(false)
                        }
                        Err(FenceError::Driver(ref error)) if is_transient(error) => continue,
                        Ok(true) => unreachable!(),
                        Err(error) => return Err(ownership_unavailable(error.into_store())),
                    }
                }
            }
        }
        Err(OwnershipError::Unavailable(
            "inspection park transaction did not settle".into(),
        ))
    })
    .await
    .map_err(unavailable)?;
    if result {
        tracing::warn!(
            agent_hash = %agent_hash(&claimed.id), epoch, park_id = %id,
            step = claimed.work.step.operation_name(), phase = failure.phase.as_str(),
            reason = failure.reason.as_str(), http_status = failure.http_status,
            "Agent runtime inspection parked"
        );
    }
    Ok(result)
}

async fn block_work(
    store: &MongoSandboxStore,
    claimed: &Agent,
    binding: &RuntimeBinding,
    agent_filter: &Document,
    marker: &Document,
    session: &mut ClientSession,
) -> Result<bool, FenceError> {
    let parent_filter = doc! {"_id": claimed.sandbox_id.as_str(), "owner": claimed.owner.as_str()};
    let Some(parent) = store
        .sandboxes
        .find_one(parent_filter.clone())
        .session(&mut *session)
        .await
        .map_err(FenceError::Driver)?
    else {
        return Ok(false);
    };
    let sandbox = super::sandboxes::read_sandbox(&parent).map_err(FenceError::Domain)?;
    if sandbox.runtime.as_ref() != Some(binding) {
        return Ok(false);
    }
    let parent_binding = parent
        .get_document("record")
        .and_then(|record| record.get_document("runtime"))
        .map_err(|error| FenceError::Domain(SandboxStoreError::Corrupt(error.to_string())))?;
    let mut parent_filter = parent_filter;
    parent_filter.insert(
        "revision",
        parent.get("revision").cloned().unwrap_or(Bson::Null),
    );
    if store
        .sandboxes
        .update_one(
            parent_filter,
            doc! {"$inc": {"runtime_writer_claim_fence": 1_i64}},
        )
        .session(&mut *session)
        .await
        .map_err(FenceError::Driver)?
        .matched_count
        != 1
    {
        return Ok(false);
    }
    let mut marker = marker.clone();
    marker.insert("binding", parent_binding.clone());
    let Some(agent) = store
        .agents
        .find_one(agent_filter.clone())
        .projection(doc! {FIELD: 1})
        .session(&mut *session)
        .await
        .map_err(FenceError::Driver)?
    else {
        return Ok(false);
    };
    let had_probe = agent.get_document(FIELD).is_ok_and(|prior| {
        (prior.contains_key("probe_at") || prior.get_bool("probe_spent").unwrap_or(false))
            && ["step", "ready_at", "attempts", "binding"]
                .iter()
                .all(|field| prior.get(field) == marker.get(field))
    });
    if had_probe {
        marker.insert("probe_spent", true);
    }
    let mut update = doc! {"$literal": marker.clone()};
    if !had_probe
        && matches!(
            marker.get_str("reason"),
            Ok("deadline" | "transport_unavailable")
        )
    {
        // A repeated failure after this probe stays parked for operator recovery.
        update = doc! {"$mergeObjects": [update, {"probe_at": {"$add": ["$$NOW", 60_000_i64]}}]};
    }
    let changed = store
        .agents
        .update_one(agent_filter.clone(), vec![doc! {"$set": {FIELD: update}}])
        .session(&mut *session)
        .await
        .map_err(FenceError::Driver)?;
    Ok(changed.matched_count == 1)
}

fn agent_hash(agent: &AgentId) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(agent.as_str().as_bytes()))
}

fn unavailable(error: impl std::fmt::Display) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

fn ownership_unavailable(error: impl std::fmt::Display) -> OwnershipError {
    OwnershipError::Unavailable(error.to_string())
}
