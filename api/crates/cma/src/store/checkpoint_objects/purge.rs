use futures_util::TryStreamExt;
use mongodb::bson::{doc, DateTime, Document};

use super::model::{read_manifest, ArchiveState, ManifestDoc};
use super::{unavailable, AdapterResult, MongoCheckpointObjectStore};
use crate::store::schema::DOCUMENT_SCHEMA_VERSION;

const PAGE_SIZE: i64 = 64;

pub(super) async fn purge_sandbox(
    store: &MongoCheckpointObjectStore,
    sandbox_id: &str,
) -> AdapterResult<()> {
    store
        .gates
        .update_one(
            doc! { "_id": sandbox_id, "document_schema_version": DOCUMENT_SCHEMA_VERSION },
            doc! {
                "$set": { "retired": true }, "$inc": { "serial": 1_i64 },
            },
        )
        .upsert(true)
        .await
        .map_err(|_| unavailable("checkpoint sandbox retirement gate failed"))?;
    store
        .manifests
        .update_many(
            doc! { "sandbox_id": sandbox_id, "state": "published" },
            doc! { "$set": { "state": "deleting", "cleanup_after": DateTime::now() } },
        )
        .await
        .map_err(|_| unavailable("checkpoint sandbox archive retirement failed"))?;
    store
        .manifests
        .update_many(
            doc! { "sandbox_id": sandbox_id, "state": "uploading" },
            doc! { "$set": { "state": "deleting" } },
        )
        .await
        .map_err(|_| unavailable("checkpoint sandbox upload retirement failed"))?;
    let documents = scan(store, doc! { "sandbox_id": sandbox_id }).await?;
    for document in documents {
        let manifest = read_manifest(document)?;
        if manifest.sandbox_id != sandbox_id {
            return Err(unavailable("checkpoint purge ownership is corrupt"));
        }
        cleanup_one(store, &manifest, DateTime::now()).await?;
    }
    if store
        .manifests
        .find_one(doc! { "sandbox_id": sandbox_id })
        .await
        .map_err(|_| unavailable("checkpoint purge reconciliation failed"))?
        .is_some()
    {
        Err(unavailable("checkpoint archive purge has pending work"))
    } else {
        Ok(())
    }
}

pub(super) async fn cleanup_due(
    store: &MongoCheckpointObjectStore,
    now: DateTime,
) -> AdapterResult<usize> {
    let documents = scan(
        store,
        doc! { "state": { "$in": ["uploading", "deleting"] }, "cleanup_after": { "$lte": now } },
    )
    .await?;
    let mut completed = 0;
    let mut error = None;
    for document in documents {
        let manifest = read_manifest(document)?;
        match cleanup_one(store, &manifest, now).await {
            Ok(true) => completed += 1,
            Ok(false) => (),
            Err(failure) => {
                error.get_or_insert(failure);
            }
        }
    }
    error.map_or(Ok(completed), Err)
}

async fn scan(
    store: &MongoCheckpointObjectStore,
    filter: Document,
) -> AdapterResult<Vec<Document>> {
    store
        .manifests
        .find(filter)
        .sort(doc! { "cleanup_after": 1, "_id": 1 })
        .limit(PAGE_SIZE)
        .await
        .map_err(|_| unavailable("checkpoint archive cleanup scan failed"))?
        .try_collect()
        .await
        .map_err(|_| unavailable("checkpoint archive cleanup scan failed"))
}

async fn cleanup_one(
    store: &MongoCheckpointObjectStore,
    manifest: &ManifestDoc,
    now: DateTime,
) -> AdapterResult<bool> {
    if manifest.state == ArchiveState::Published {
        return Ok(false);
    }
    // Freeze abandoned stages before external deletion; a late publisher must lose.
    let changed = store
        .manifests
        .update_one(
            doc! { "_id": &manifest.id, "state": { "$in": ["uploading", "deleting"] } },
            doc! { "$set": { "state": "deleting" } },
        )
        .await
        .map_err(|_| unavailable("checkpoint archive cleanup claim failed"))?;
    if changed.matched_count != 1 {
        return Ok(false);
    }
    store.objects.delete(&manifest.object_key).await?;
    if !manifest.upload_confirmed {
        // A client deadline cannot bound remote completion. Preserve ownership
        // of this exact key even after an observed absence lets purge finish.
        return super::tombstones::retain(store, manifest, now).await;
    }
    let removed = store
        .manifests
        .delete_one(
            doc! { "_id": &manifest.id, "state": "deleting", "object_key": &manifest.object_key },
        )
        .await
        .map_err(|_| unavailable("checkpoint archive cleanup confirmation failed"))?;
    Ok(removed.deleted_count == 1)
}
