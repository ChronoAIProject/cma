//! Preserve an exact generation image across unresolved dispatches and rollouts.
use layer2_core::sandbox::Sandbox;

use super::*;
use crate::store::sandbox_mongo::MongoSandboxStore;

const DISPATCH: &str = "initial_runtime_dispatch";

pub(in crate::store) async fn reserve_initial_runtime_image(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    generation: u64,
    image: &str,
) -> Result<String, SandboxStoreError> {
    if store.released_lanes.is_none() {
        return Ok(image.into());
    }
    let generation = revision(generation)?;
    let filter = doc! {"_id": sandbox.id.as_str(), "revision": revision(sandbox.revision)?};
    let row = store
        .sandboxes
        .find_one(filter.clone())
        .await
        .map_err(unavailable)?
        .ok_or(SandboxStoreError::Conflict)?;
    if let Ok(dispatch) = row.get_document(DISPATCH) {
        let known = dispatch.get_i64("generation").map_err(|_| corrupt())?;
        if known == generation {
            return dispatch
                .get_str("image")
                .map(str::to_owned)
                .map_err(|_| corrupt());
        }
        if known > generation {
            return Err(SandboxStoreError::Conflict);
        }
    }
    // A compatible first claim persists its pin in the claim transaction.
    // An absent pin here is an unresolved pre-upgrade dispatch.
    tracing::warn!(sandbox_id = %sandbox.id, generation, "unbound initial runtime dispatch needs reconciliation");
    Err(SandboxStoreError::Conflict)
}

pub(in crate::store) fn pin_first_dispatch(
    store: &MongoSandboxStore,
    current: &mongodb::bson::Document,
    parent: &Sandbox,
    fields: &mut mongodb::bson::Document,
) -> Result<(), SandboxStoreError> {
    use layer2_core::sandbox::SandboxWorkStep;

    if store.released_lanes.is_none()
        || current.contains_key(DISPATCH)
        || parent.work.step != SandboxWorkStep::ProvisionRuntime
        || parent.work.next_fence != 1
        || parent.work.attempts != 0
        || parent.profile.content.runtime_image.is_some()
    {
        return Ok(());
    }
    let image = store
        .managed_runtime_version
        .as_deref()
        .ok_or_else(corrupt)?;
    fields.insert(
        DISPATCH,
        doc! {
            "generation": revision(parent.lifecycle.runtime_generation_high_water.max(1))?,
            "image": image,
        },
    );
    Ok(())
}

pub(in crate::store) fn pin_replacement_dispatch(
    store: &MongoSandboxStore,
    parent: &Sandbox,
    next: &Sandbox,
    fields: &mut mongodb::bson::Document,
) -> Result<(), SandboxStoreError> {
    use layer2_core::sandbox::SandboxWorkStep;

    let Some(original) = &parent.runtime else {
        return Ok(());
    };
    let generation = next.lifecycle.runtime_generation_high_water;
    if store.released_lanes.is_none()
        || parent.work.step != SandboxWorkStep::ProvisionRuntime
        || next.work.step != SandboxWorkStep::ProvisionRuntime
        || next.runtime.is_some()
        || next.profile.content.runtime_image.is_some()
        || next
            .lifecycle
            .home
            .as_ref()
            .is_some_and(|home| home.native_pool.is_some())
        || parent
            .lifecycle
            .runtime_generation_high_water
            .checked_add(1)
            != Some(generation)
    {
        return Ok(());
    }
    // The prior binding names the exact non-native image. The replacement
    // generation starts here, so store its pin before any worker can dispatch.
    fields.insert(
        DISPATCH,
        doc! {
            "generation": revision(generation)?,
            "image": &original.runtime_version,
        },
    );
    Ok(())
}

impl MongoReleasedRuntimes {
    pub(crate) async fn unresolved_legacy_dispatches(
        &self,
        session: &mut ClientSession,
    ) -> Result<bool, SandboxStoreError> {
        let sandboxes: Collection<mongodb::bson::Document> =
            majority_collection(&self.database, "sandboxes");
        let row = sandboxes.find_one(doc! {
            "record.work.step.kind": "provision_runtime",
            "record.profile.content.runtime_image": mongodb::bson::Bson::Null,
            "record.lifecycle.home.native_pool": mongodb::bson::Bson::Null,
            "record.runtime": mongodb::bson::Bson::Null,
            "$or": [{"record.work.next_fence": {"$gt": 1}}, {"record.work.attempts": {"$gt": 0}}],
            DISPATCH: {"$exists":false},
        }).projection(doc! {"_id": 1}).session(session).await.map_err(unavailable)?;
        Ok(row.is_some())
    }
}

fn corrupt() -> SandboxStoreError {
    SandboxStoreError::Corrupt("invalid retained initial runtime dispatch".into())
}
