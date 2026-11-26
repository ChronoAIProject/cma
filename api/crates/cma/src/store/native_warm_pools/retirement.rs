//! Retirement needs current release authority plus every durable allocation.
use super::*;
use layer2_core::sandbox::{Sandbox, SandboxStatus, WarmSandbox, WarmSandboxState};
use layer3_adapter::HomeVolumeAttachment;

mod eligibility;
pub(super) mod machine_selection;
use eligibility::{owned_home, warm_blocks};

pub(super) async fn fence_machine_selection(
    store: &MongoNativeWarmPools,
    session: &mut mongodb::ClientSession,
) -> Result<(), WarmPoolError> {
    machine_selection::selection_fence(store, session)
        .await
        .map_err(|_| WarmPoolError::Unavailable)
}

const MAX_RECORDS: i64 = 10_000;

pub(super) fn release_scope(store: &MongoNativeWarmPools, machine: bool) -> Document {
    let mut scope = store.scope();
    scope.insert(
        "machine",
        if machine {
            mongodb::bson::Bson::Boolean(true)
        } else {
            doc! {"$ne": true}.into()
        },
    );
    scope
}

pub(super) fn release_order(machine: bool) -> Document {
    let mut order = doc! {"activated_at": -1};
    if machine {
        order.insert("machine_plan_activated_at", -1);
    }
    order
}

pub(super) async fn machine_selection_ambiguous(
    store: &MongoNativeWarmPools,
    selected: &PoolTarget,
) -> Result<bool, WarmPoolError> {
    let Some(at) = selected.machine_plan_activated_at else {
        return Ok(true);
    };
    let mut scope = release_scope(store, true);
    scope.extend(doc! {"activated_at": selected.activated_at,
    "machine_plan_activated_at": at, "_id": {"$ne": &selected.id}});
    Ok(store
        .targets
        .find_one(scope)
        .await
        .map_err(|_| WarmPoolError::Unavailable)?
        .is_some())
}

async fn authorized(
    store: &MongoNativeWarmPools,
    target: &PoolTarget,
    image: &str,
) -> Result<bool, WarmPoolError> {
    let observed = store
        .targets
        .find_one(fence(target))
        .await
        .map_err(|_| WarmPoolError::Unavailable)?
        .ok_or(WarmPoolError::Conflict)?;
    if observed.machine_plan_activated_at != target.machine_plan_activated_at {
        return Err(WarmPoolError::Conflict);
    }
    // Tombstones retain release order even after old capacity is removed.
    let mut scope = release_scope(store, target.machine);
    scope.insert("activated_at", doc! {"$type":"date"});
    let latest_row = store
        .targets
        .find_one(scope)
        .sort(release_order(target.machine))
        .await
        .map_err(|_| WarmPoolError::Unavailable)?;
    let Some(selected) = latest_row else {
        return Ok(false);
    };
    let Some(latest) = selected.activated_at else {
        return Ok(false);
    };
    if selected.plan.spec.image.uri != image || selected.retiring || selected.retired {
        return Ok(false);
    }
    let same_image = target.plan.spec.image.uri == image;
    let exact_machine_retirement = target.machine && same_image;
    let settle_from = if exact_machine_retirement {
        let Some(selected_at) = selected.machine_plan_activated_at else {
            return Ok(false);
        };
        if target.id == selected.id
            || target.activated_at.is_some_and(|at| at != latest)
            || target
                .machine_plan_activated_at
                .is_some_and(|at| at >= selected_at)
        {
            return Ok(false);
        }
        selected_at
    } else {
        if target.activated_at.is_some_and(|at| at >= latest) {
            return Ok(false);
        }
        latest
    };
    // Allow rollout and bounded pre-selection work to settle; ownership safety
    // still comes from the complete durable scan and provider graph proof.
    if DateTime::now()
        .timestamp_millis()
        .saturating_sub(settle_from.timestamp_millis())
        < crate::warm_pools::RELEASE_SETTLE.as_millis() as i64
    {
        return Ok(false);
    }
    // A same-millisecond tie between different releases is ambiguous. Retired
    // Desktop plans on the selected image do not invalidate its selected plan.
    let mut conflict = release_scope(store, target.machine);
    conflict.insert("activated_at", latest);
    if target.machine && selected.machine_plan_activated_at.is_some() {
        conflict.insert("plan.spec.image.uri", doc! {"$ne": image});
        if machine_selection_ambiguous(store, &selected).await? {
            return Ok(false);
        }
    } else {
        conflict.insert(
            "$or",
            vec![
                doc! {"retiring":true},
                doc! {"retired":true},
                doc! {"plan.spec.image.uri":{"$ne":image}},
            ],
        );
    }
    Ok(store
        .targets
        .find_one(conflict)
        .await
        .map_err(|_| WarmPoolError::Unavailable)?
        .is_none())
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
    Ok(scan(store, target, image, false).await?.map(|e| e.homes))
}

pub(super) async fn drain_homes(
    store: &MongoNativeWarmPools,
    target: &PoolTarget,
    image: &str,
) -> Result<Option<NativePoolDrain>, WarmPoolError> {
    scan(store, target, image, true).await
}

async fn scan(
    store: &MongoNativeWarmPools,
    target: &PoolTarget,
    image: &str,
    draining: bool,
) -> Result<Option<NativePoolDrain>, WarmPoolError> {
    if !authorized(store, target, image).await? {
        return Ok(deferred(target, "release_settling"));
    }
    // Inspect the operation first. If it has finished, the later Sandbox
    // scan sees any home it created before publishing that outcome.
    if store.released_lanes.is_some()
        && crate::store::agent_lifecycle_operations::released::protects_pool(
            &store.database,
            target.machine,
            &target.plan.spec.image.uri,
            (target.machine && target.plan.spec.image.uri == image).then_some(
                layer3_adapter::SandboxCapacity {
                    cpu_millis: target.plan.spec.cpu_millis,
                    memory_mib: target.plan.spec.memory_mib,
                },
            ),
        )
        .await
        .map_err(|_| WarmPoolError::Unavailable)?
    {
        return Ok(deferred(target, "lifecycle_admission"));
    }
    let mut evidence = NativePoolDrain::default();
    for document in documents(store, "sandboxes", doc! {}).await? {
        let sandbox = crate::store::sandboxes::read_sandbox(&document)
            .map_err(|_| WarmPoolError::Unavailable)?;
        match owned_home(&sandbox, target, image)? {
            eligibility::Owned::Blocked => {
                if !draining {
                    return Ok(deferred(target, "sandbox_in_use"));
                }
                if let Some(home) = eligibility::settled_native_home(&sandbox, target)? {
                    evidence.homes.push(home);
                    continue;
                }
                let Some(native) = sandbox
                    .lifecycle
                    .home
                    .as_ref()
                    .and_then(|home| home.native_pool.as_ref())
                else {
                    return Ok(deferred(target, "sandbox_unsettled"));
                };
                if target.binding.as_ref() != Some(&native.request.pool) {
                    return Ok(deferred(target, "sandbox_unsettled"));
                }
                let Some(claim) = native.claim.as_ref().filter(|claim| {
                    !native.cleaned
                        && native.cleanup.is_none()
                        && claim.request == native.request
                        && claim.sandbox_name.is_some()
                }) else {
                    return Ok(deferred(target, "sandbox_unsettled"));
                };
                evidence.claims.push(claim.clone());
            }
            eligibility::Owned::Retained(home) => evidence.homes.push(*home),
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
            if warm_blocks(&warm, target, image)? {
                return Ok(deferred(target, "prepared_reservation"));
            }
        }
    }
    for document in documents(store, "warm_sandboxes", doc! {}).await? {
        let warm = crate::store::warm_sandboxes::read(&document)
            .map_err(|_| WarmPoolError::Unavailable)?;
        if warm_blocks(&warm, target, image)? {
            return Ok(deferred(target, "legacy_warm_sandbox"));
        }
    }
    // This runs again after the provider's admission marker. Every allocation
    // persists its reservation before selection/Claim I/O; the fresh scan thus
    // catches work that crossed the earlier selection before the marker.
    if !authorized(store, target, image).await? {
        return Ok(deferred(target, "release_settling"));
    }
    Ok(Some(evidence))
}

/// Retirement used to defer silently, leaving obsolete capacity running with
/// no sign of why. Name the blocking class so a stuck record is diagnosable.
fn deferred(target: &PoolTarget, reason: &'static str) -> Option<NativePoolDrain> {
    tracing::info!(
        target: "cma::warm_pools",
        pool = %target.id,
        machine = target.machine,
        reason,
        "native warm pool retirement deferred"
    );
    None
}

pub(super) async fn begin(
    store: &MongoNativeWarmPools,
    target: &PoolTarget,
) -> Result<PoolTarget, WarmPoolError> {
    if target.retiring {
        return Ok(target.clone());
    }
    let mut filter = fence(target);
    filter.insert("preparation", doc! {"$exists": false});
    filter.insert(
        "activated_at",
        mongodb::bson::serialize_to_bson(&target.activated_at)
            .map_err(|_| WarmPoolError::Unavailable)?,
    );
    filter.insert(
        "machine_plan_activated_at",
        mongodb::bson::serialize_to_bson(&target.machine_plan_activated_at)
            .map_err(|_| WarmPoolError::Unavailable)?,
    );
    let updated = store
        .targets
        .find_one_and_update(
            filter,
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
