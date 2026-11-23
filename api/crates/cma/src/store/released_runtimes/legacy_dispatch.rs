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
    // An older worker could have dispatched under a preceding lease. Without
    // its receipt or exact image, a second dispatch can create a different runtime.
    if sandbox.work.next_fence != 2 || sandbox.work.attempts != 0 {
        tracing::warn!(sandbox_id = %sandbox.id, generation, "unbound initial runtime dispatch needs reconciliation");
        return Err(SandboxStoreError::Conflict);
    }
    let mut filter = filter;
    filter.insert(
        "record.work.lease.expires_at",
        doc! {"$gt": layer2_core::now_rfc3339()},
    );
    match row.get(DISPATCH) {
        Some(previous) => filter.insert(DISPATCH, previous.clone()),
        None => filter.insert(DISPATCH, doc! {"$exists":false}),
    };
    let result = store
        .sandboxes
        .update_one(
            filter,
            doc! {
                "$set": {DISPATCH: {"generation": generation, "image": image}},
            },
        )
        .await
        .map_err(unavailable)?;
    if result.matched_count != 1 {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(image.into())
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
