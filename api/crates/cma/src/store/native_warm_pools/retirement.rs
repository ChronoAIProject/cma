//! Retirement needs current release authority plus every durable allocation.
use super::*;
use layer2_core::sandbox::{Sandbox, SandboxStatus, WarmSandbox, WarmSandboxState};
use layer3_adapter::HomeVolumeAttachment;

mod eligibility;
use eligibility::{owned_home, warm_blocks};

const MAX_RECORDS: i64 = 10_000;

pub(super) async fn activate(store: &MongoNativeWarmPools, id: &str) -> Result<(), WarmPoolError> {
    let mut filter = store.scope();
    filter.extend(doc! {"_id":id,"activated_at":mongodb::bson::Bson::Null,"retiring":{"$ne":true},"retired":{"$ne":true}});
    // First activation is immutable. Restarting an older API cannot promote its
    // old image above a newer release and begin deleting that release's pools.
    store
        .targets
        .update_one(filter, doc! {"$currentDate":{"activated_at":true}})
        .await
        .map_err(|_| WarmPoolError::Unavailable)?;
    let row = store
        .find(id)
        .await?
        .filter(|row| row.activated_at.is_some() && !row.retiring && !row.retired)
        .ok_or(WarmPoolError::Conflict)?;
    let mut scope = store.scope();
    scope.insert("activated_at", doc! {"$type":"date"});
    let latest = store
        .targets
        .find_one(scope)
        .sort(doc! {"activated_at":-1})
        .await
        .map_err(|_| WarmPoolError::Unavailable)?
        .ok_or(WarmPoolError::Conflict)?;
    // API rollbacks keep the released runtime pointer. An older process must
    // not start serving new allocations with a previously superseded default.
    if latest.plan.spec.image.uri != row.plan.spec.image.uri || latest.retiring || latest.retired {
        return Err(WarmPoolError::Conflict);
    }
    let mut ambiguous = store.scope();
    ambiguous.extend(doc! {"activated_at":latest.activated_at,
    "plan.spec.image.uri":{"$ne":&row.plan.spec.image.uri}});
    if store
        .targets
        .find_one(ambiguous)
        .await
        .map_err(|_| WarmPoolError::Unavailable)?
        .is_some()
    {
        return Err(WarmPoolError::Conflict);
    }
    Ok(())
}

async fn authorized(
    store: &MongoNativeWarmPools,
    target: &PoolTarget,
    image: &str,
) -> Result<bool, WarmPoolError> {
    if store
        .targets
        .find_one(fence(target))
        .await
        .map_err(|_| WarmPoolError::Unavailable)?
        .is_none()
    {
        return Err(WarmPoolError::Conflict);
    }
    // Tombstones retain release order even after old capacity is removed.
    let mut scope = store.scope();
    scope.insert("activated_at", doc! {"$type":"date"});
    let latest_row = store
        .targets
        .find_one(scope)
        .sort(doc! {"activated_at":-1})
        .await
        .map_err(|_| WarmPoolError::Unavailable)?;
    let Some(latest) = latest_row.and_then(|row| row.activated_at) else {
        return Ok(false);
    };
    // Allow rollout and bounded pre-selection work to settle; ownership safety
    // still comes from the complete durable scan and provider graph proof.
    if DateTime::now()
        .timestamp_millis()
        .saturating_sub(latest.timestamp_millis())
        < 120_000
    {
        return Ok(false);
    }
    // A same-millisecond tie between different releases is ambiguous: defer.
    let mut conflict = store.scope();
    conflict.extend(doc! {"activated_at":latest,"$or":[
        {"retiring":true},{"retired":true},{"plan.spec.image.uri":{"$ne":image}}
    ]});
    Ok(store
        .targets
        .find_one(conflict)
        .await
        .map_err(|_| WarmPoolError::Unavailable)?
        .is_none()
        && target.activated_at.is_none_or(|at| at < latest))
}

async fn documents(
    store: &MongoNativeWarmPools,
    name: &str,
    filter: Document,
) -> Result<Vec<Document>, WarmPoolError> {
    let rows: Vec<_> = majority_collection::<Document>(&store.database, name)
        .find(filter)
        .sort(doc! {"_id":1})
        .limit(MAX_RECORDS + 1)
        .await
        .map_err(|_| WarmPoolError::Unavailable)?
        .try_collect()
        .await
        .map_err(|_| WarmPoolError::Unavailable)?;
    if rows.len() > MAX_RECORDS as usize {
        return Err(WarmPoolError::Unavailable);
    }
    Ok(rows)
}

pub(super) async fn homes(
    store: &MongoNativeWarmPools,
    target: &PoolTarget,
    image: &str,
) -> Result<Option<Vec<HomeVolumeAttachment>>, WarmPoolError> {
    if !authorized(store, target, image).await? {
        return Ok(None);
    }
    let mut homes = Vec::new();
    for document in documents(store, "sandboxes", doc! {}).await? {
        let sandbox = crate::store::sandboxes::read_sandbox(&document)
            .map_err(|_| WarmPoolError::Unavailable)?;
        match owned_home(&sandbox, target, image)? {
            eligibility::Owned::Blocked => return Ok(None),
            eligibility::Owned::Retained(home) => homes.push(*home),
            eligibility::Owned::Unrelated => (),
        }
    }
    for document in documents(
        store,
        "native_prepared_sandboxes",
        doc! {"kind":"reservation"},
    )
    .await?
    {
        if let Some(warm) = crate::store::native_prepared::unowned_for_retirement(&document)
            .map_err(|_| WarmPoolError::Unavailable)?
        {
            if warm_blocks(&warm, target) {
                return Ok(None);
            }
        }
    }
    for document in documents(store, "warm_sandboxes", doc! {}).await? {
        let warm = crate::store::warm_sandboxes::read(&document)
            .map_err(|_| WarmPoolError::Unavailable)?;
        if warm_blocks(&warm, target) {
            return Ok(None);
        }
    }
    // This runs again after the provider's admission marker. Every allocation
    // persists its reservation before selection/Claim I/O; the fresh scan thus
    // catches work that crossed the earlier selection before the marker.
    if !authorized(store, target, image).await? {
        return Ok(None);
    }
    Ok(Some(homes))
}

pub(super) async fn begin(
    store: &MongoNativeWarmPools,
    target: &PoolTarget,
) -> Result<PoolTarget, WarmPoolError> {
    if target.retiring {
        return Ok(target.clone());
    }
    let updated = store
        .targets
        .find_one_and_update(
            fence(target),
            doc! {
                "$set":{"retiring":true,"target_replicas":0_i64},"$inc":{"revision":1_i64}
            },
        )
        .return_document(ReturnDocument::After)
        .await
        .map_err(|_| WarmPoolError::Unavailable)?;
    updated.ok_or(WarmPoolError::Conflict).and_then(validate)
}

pub(super) async fn remove(
    store: &MongoNativeWarmPools,
    target: &PoolTarget,
) -> Result<(), WarmPoolError> {
    let mut filter = fence(target);
    filter.extend(doc! {"retiring":true,"target_replicas":0_i64});
    // Keep the immutable activation/identity tombstone. Physically removing it
    // would let an old API recreate its deterministic target with a fresh date.
    store.check_fenced_write(store.targets.update_one(filter, doc! {"$set":{
        "retired":true,"lease":mongodb::bson::Bson::Null,"lease_until":DateTime::from_millis(0),
        "error":mongodb::bson::Bson::Null,"applied_revision":target.revision
    }}).await).await
}

#[cfg(test)]
mod eligibility_tests;
#[cfg(test)]
mod tests;
