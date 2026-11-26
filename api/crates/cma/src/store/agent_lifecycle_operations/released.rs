use futures_util::TryStreamExt;
use layer1_public::rest::AgentLifecycleCreateRequest;
use layer2_core::released_runtime::ReleasedLane;
use layer2_core::sandbox::{store::SandboxStoreError, Sandbox, SandboxOrigin};
use layer3_adapter::ReleasedRuntime;
use mongodb::{bson::Document, Database};

use super::{doc, majority_collection, schema, LifecycleBehavior, LifecycleRecord, COLLECTION};
use crate::store::released_runtimes::MongoReleasedRuntimes;

pub(crate) fn managed_lifecycle_release_lane(
    request: &AgentLifecycleCreateRequest,
    behavior: LifecycleBehavior,
) -> Option<ReleasedLane> {
    if request.image.is_some() || request.workspace_id.is_some() || request.sandbox_id.is_some() {
        return None;
    }
    if request.machine_included {
        Some(ReleasedLane::Machine)
    } else {
        (request.desktop
            || matches!(
                behavior,
                LifecycleBehavior::RepositoryChoice | LifecycleBehavior::PublicRepository
            ))
        .then_some(ReleasedLane::Agent)
    }
}

pub(in crate::store) async fn admitted_release(
    database: &Database,
    lanes: &MongoReleasedRuntimes,
    sandbox: &Sandbox,
) -> Result<Option<ReleasedRuntime>, SandboxStoreError> {
    let SandboxOrigin::Lifecycle { operation_id } = &sandbox.origin else {
        return Ok(None);
    };
    let Some(image) = &sandbox.profile.content.runtime_image else {
        return Ok(None);
    };
    let operations = majority_collection::<LifecycleRecord>(database, COLLECTION);
    let Some(operation) = operations
        .find_one(doc! { "_id": operation_id, "owner": sandbox.owner.as_str() })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
    else {
        return Ok(None);
    };
    if operation.canary_admission.is_some() || super::canary_key::is_reserved(&operation.key) {
        return Ok(None);
    }
    let lane = managed_lifecycle_release_lane(&operation.input.request, operation.input.behavior)
        .filter(|_| operation.input.resolved_image.as_ref() == Some(image));
    let lane = match (operation.released_lane, lane) {
        (Some(recorded), Some(lane))
            if recorded == lane
                && operation.document_schema_version == schema::DOCUMENT_SCHEMA_VERSION =>
        {
            lane
        }
        (Some(_), _) => {
            return Err(SandboxStoreError::Corrupt(
                "lifecycle release admission differs from its Sandbox".into(),
            ));
        }
        // A pre-marker desktop receipt needs proof that its exact image
        // was released in this deployment and lane.
        (None, Some(lane))
            if operation.document_schema_version == schema::DOCUMENT_SCHEMA_VERSION
                && lanes.has_served_image(lane, image).await? =>
        {
            lane
        }
        (None, _) => return Ok(None),
    };
    // The lane fence admitted this exact image. The Sandbox profile still
    // renders the request, and this snapshot supplies its verified metadata.
    let (agent, machine) = match lane {
        ReleasedLane::Machine => (String::new(), Some(image.clone())),
        ReleasedLane::Agent => (image.clone(), None),
    };
    Ok(Some(ReleasedRuntime {
        revision: 0,
        lane_revisions: None,
        agent,
        machine,
    }))
}

pub(in crate::store) async fn protects_pool(
    database: &Database,
    machine: bool,
    image: &str,
    capacity: Option<layer3_adapter::SandboxCapacity>,
) -> mongodb::error::Result<bool> {
    // Include old admissions made before the durable lane marker existed.
    let mut filter = doc! {
        "status": { "$in": ["queued", "running", "ready", "waiting_for_connection"] },
        "input.resolved_image": image,
        "canary_admission": mongodb::bson::Bson::Null,
        "input.request.image": mongodb::bson::Bson::Null,
        "input.request.workspace_id": mongodb::bson::Bson::Null,
        "input.request.sandbox_id": mongodb::bson::Bson::Null,
    };
    if machine {
        filter.insert("input.request.machine_included", true);
    } else {
        filter.insert("input.request.machine_included", doc! { "$ne": true });
        filter.insert(
            "$or",
            [
                doc! {"input.request.desktop": true},
                doc! {"input.behavior": {"$in": ["repository_choice", "public_repository"]}},
            ],
        );
    }
    if let Some(capacity) = capacity {
        // The admitted capacity is immutable. A differently sized operation
        // on the same released Desktop image cannot select this exact pool.
        // Legacy records without a pin must still defer retirement.
        const MAX_ADMISSIONS: i64 = 10_000;
        let mut rows = majority_collection::<LifecycleRecord>(database, COLLECTION)
            .find(filter)
            .limit(MAX_ADMISSIONS + 1)
            .await?;
        let mut count = 0;
        while let Some(row) = rows.try_next().await? {
            count += 1;
            if count > MAX_ADMISSIONS || row.input.capacity.is_none_or(|pin| pin == capacity) {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    Ok(majority_collection::<Document>(database, COLLECTION)
        .find_one(filter)
        .projection(doc! { "_id": 1 })
        .await?
        .is_some())
}
