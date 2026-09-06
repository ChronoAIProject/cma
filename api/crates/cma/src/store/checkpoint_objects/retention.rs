use layer2_core::sandbox::SandboxCheckpointRef;
use mongodb::bson::{doc, DateTime};
use mongodb::ClientSession;

use super::model::read_manifest;
use super::MongoCheckpointObjectStore;

pub(in crate::store) enum RetentionError {
    Corrupt(&'static str),
    Driver(mongodb::error::Error),
}

pub(in crate::store) async fn prune_evicted(
    store: &MongoCheckpointObjectStore,
    sandbox_id: &str,
    checkpoint: &SandboxCheckpointRef,
    session: &mut ClientSession,
) -> Result<(), RetentionError> {
    let document = store.manifests.find_one(doc! { "sandbox_id": sandbox_id, "object_reference": &checkpoint.object_reference, "state": "published" })
        .session(&mut *session).await.map_err(RetentionError::Driver)?;
    let Some(document) = document else {
        return Ok(());
    };
    let manifest = read_manifest(document)
        .map_err(|_| RetentionError::Corrupt("evicted checkpoint metadata is corrupt"))?;
    if manifest.sandbox_id != sandbox_id
        || manifest.object_reference != checkpoint.object_reference
        || manifest.sha256 != checkpoint.sha256
    {
        return Err(RetentionError::Corrupt(
            "evicted checkpoint identity is corrupt",
        ));
    }
    // Publication authority and durable external-deletion intent move together.
    let result = store
        .manifests
        .update_one(
            doc! { "_id": &manifest.id, "state": "published" },
            doc! { "$set": { "state": "deleting", "cleanup_after": DateTime::now() } },
        )
        .session(session)
        .await
        .map_err(RetentionError::Driver)?;
    if result.matched_count == 1 {
        Ok(())
    } else {
        Err(RetentionError::Corrupt(
            "evicted checkpoint changed during retention",
        ))
    }
}
