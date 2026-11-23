use layer1_public::rest::AgentLifecycleCreateRequest;
use layer2_core::released_runtime::ReleasedLane;
use layer2_core::sandbox::{store::SandboxStoreError, Sandbox, SandboxOrigin};
use layer3_adapter::ReleasedRuntime;
use mongodb::{bson::Document, Database};

use super::{doc, majority_collection, schema, LifecycleRecord, COLLECTION};

pub(crate) fn managed_lifecycle_release_lane(
    request: &AgentLifecycleCreateRequest,
) -> Option<ReleasedLane> {
    if request.image.is_some() || request.workspace_id.is_some() || request.sandbox_id.is_some() {
        return None;
    }
    if request.machine_included {
        Some(ReleasedLane::Machine)
    } else {
        request.desktop.then_some(ReleasedLane::Agent)
    }
}

pub(in crate::store) async fn admitted_release(
    database: &Database,
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
        .find_one(doc! { "_id": operation_id, "owner": sandbox.owner.as_str(), "released_lane": { "$exists": true } })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
    else {
        return Ok(None);
    };
    let lane = managed_lifecycle_release_lane(&operation.input.request);
    if operation.document_schema_version != schema::DOCUMENT_SCHEMA_VERSION
        || lane.is_none()
        || operation.released_lane != lane
        || operation.input.resolved_image.as_ref() != Some(image)
    {
        return Err(SandboxStoreError::Corrupt(
            "lifecycle release admission differs from its Sandbox".into(),
        ));
    }
    // The lane fence admitted this exact image. The Sandbox profile still
    // renders the request, and this snapshot supplies its verified metadata.
    let (agent, machine) = match lane {
        Some(ReleasedLane::Machine) => (String::new(), Some(image.clone())),
        _ => (image.clone(), None),
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
) -> mongodb::error::Result<bool> {
    // Include old admissions made before the durable lane marker existed.
    let mut filter = doc! {
        "status": { "$in": ["queued", "running", "ready", "waiting_for_connection"] },
        "input.resolved_image": image,
        "input.request.image": mongodb::bson::Bson::Null,
        "input.request.workspace_id": mongodb::bson::Bson::Null,
        "input.request.sandbox_id": mongodb::bson::Bson::Null,
    };
    if machine {
        filter.insert("input.request.machine_included", true);
    } else {
        filter.insert("input.request.machine_included", doc! { "$ne": true });
        filter.insert("input.request.desktop", true);
    }
    Ok(majority_collection::<Document>(database, COLLECTION)
        .find_one(filter)
        .projection(doc! { "_id": 1 })
        .await?
        .is_some())
}
