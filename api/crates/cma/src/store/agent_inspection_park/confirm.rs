//! Transfer a recovered step back to ordinary exact-claim effect recovery.
use super::*;
use layer2_core::agent_ownership::execution_owner;

pub(in crate::store) async fn confirm(
    store: &MongoSandboxStore,
    claimed: &Agent,
    binding: &RuntimeBinding,
) -> Result<(), SandboxStoreError> {
    if !store.agent_inspection_parking || store.agent_ownership.is_none() {
        return Ok(());
    }
    let Some(lease) = &claimed.work.lease else {
        return Ok(());
    };
    let number = |value, name| signed(value, name).map_err(SandboxStoreError::Corrupt);
    let filter = doc! {
        "_id": claimed.id.as_str(),
        format!("{FIELD}.fence"): number(claimed.work.next_fence, "inspection fence")?,
        format!("{FIELD}.retry_claimed"): true,
        format!("{FIELD}.inspection_confirmed"): {"$ne": true},
    };
    let observed = store
        .agents
        .find_one(filter.clone())
        .projection(doc! {FIELD: 1})
        .await
        .map_err(unavailable)?;
    let Some(row) = observed else {
        return Ok(());
    };
    let marker = row.get_document(FIELD).map_err(unavailable)?;
    let data = marker.get_document("binding").map_err(unavailable)?;
    let parked_binding: RuntimeBinding =
        mongodb::bson::deserialize_from_document(data.clone()).map_err(unavailable)?;
    if &parked_binding != binding {
        return Ok(());
    }
    let token = execution_owner()
        .filter(|token| token.agent_id == claimed.id)
        .ok_or(SandboxStoreError::Conflict)?;
    let revision = number(claimed.revision, "inspection revision")?;
    let epoch = number(token.epoch, "inspection owner epoch")?;
    let work_fence = number(lease.fence, "inspection work fence")?;
    let park_id = marker.get_str("id").map_err(unavailable)?;
    let exact = doc! {
        "_id": claimed.id.as_str(), "revision": revision, "record.revision": revision,
        "owner": claimed.owner.as_str(), "record.sandbox_id": claimed.sandbox_id.as_str(),
        "record.work.step": claimed.work.step.operation_name(),
        "agent_manager_work_epoch": epoch,
        "work_lease_holder": &lease.holder, "work_lease_fence": work_fence,
        "work_lease_expires_at": &lease.expires_at, FIELD: marker.clone(),
        "$expr": {"$gt": ["$work_lease_expires_at", {"$dateToString": {"date": "$$NOW", "format": "%Y-%m-%dT%H:%M:%SZ"}}]},
    };
    let parent = doc! {"_id": claimed.sandbox_id.as_str(), "owner": claimed.owner.as_str(),
    "record.runtime": data.clone()};
    bounded(async {
        let mut session = store.client.start_session().await.map_err(ownership_unavailable)?;
        start_transaction(&mut session).await.map_err(ownership_unavailable)?;
        let result: Result<bool, FenceError> = async {
            store.fence_agent_effect_transaction(&claimed.id, &mut session).await?;
            let locked = store.sandboxes.update_one(parent,
                doc! {"$inc": {"runtime_writer_claim_fence": 1_i64}})
                .session(&mut session).await.map_err(FenceError::Driver)?;
            if locked.matched_count != 1 {
                return Ok(false);
            }
            let updated = store.agents.update_one(exact,
                doc! {"$set": {format!("{FIELD}.inspection_confirmed"): true}})
                .session(&mut session).await.map_err(FenceError::Driver)?;
            Ok(updated.matched_count == 1)
        }.await;
        match result {
            Ok(true) => match commit_transaction(&mut session).await {
                CommitOutcome::Committed => Ok(()),
                CommitOutcome::Unknown(_) => {
                    let durable = store.agents.find_one(doc! {
                        "_id": claimed.id.as_str(), "revision": revision, "agent_manager_work_epoch": epoch,
                        format!("{FIELD}.id"): park_id,
                        format!("{FIELD}.inspection_confirmed"): true,
                    }).projection(doc! {"_id": 1}).await.map_err(ownership_unavailable)?;
                    if durable.is_some() { Ok(()) } else { Err(OwnershipError::Stale) }
                }
                CommitOutcome::Failed(error) | CommitOutcome::Retry(error) => Err(ownership_unavailable(error)),
            },
            other => {
                session.abort_guarded().await.map_err(ownership_unavailable)?;
                match other {
                    Err(error) => Err(ownership_unavailable(error.into_store())),
                    _ => Err(OwnershipError::Stale),
                }
            }
        }
    }).await.map_err(unavailable)
}
