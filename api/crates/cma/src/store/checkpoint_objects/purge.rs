#![allow(dead_code)]

use futures_util::TryStreamExt;
use layer2_core::sandbox::SandboxStatus;
use mongodb::bson::{doc, DateTime, Document};

use super::model::{read_manifest, ArchiveState, ManifestDoc};
use super::{unavailable, AdapterResult, MongoCheckpointObjectStore};
use crate::store::schema::DOCUMENT_SCHEMA_VERSION;

const PAGE_SIZE: i64 = 64;

pub(super) async fn purge_sandbox(
    store: &MongoCheckpointObjectStore,
    sandbox_id: &str,
) -> AdapterResult<()> {
    if channel_retained(store, sandbox_id).await? {
        return Err(super::rejected(
            "Channel checkpoint retention forbids generic sandbox purge",
        ));
    }
    retire_archives(store, sandbox_id, DateTime::now()).await?;
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

pub(super) async fn retire_deleted_sandboxes(
    store: &MongoCheckpointObjectStore,
    now: DateTime,
) -> AdapterResult<usize> {
    let candidates: Vec<Document> = store.manifests.aggregate([
        doc! { "$match": { "state": { "$in": ["published", "uploading"] } } },
        doc! { "$group": { "_id": "$sandbox_id" } },
        doc! { "$lookup": { "from": super::super::sandbox_mongo::SANDBOXES, "localField": "_id", "foreignField": "_id", "as": "sandbox" } },
        doc! { "$match": { "sandbox.status": "retired" } },
        // Filter retained roots before the page limit so they cannot starve
        // ordinary retirement cleanup, even when canonical markers were lost.
        doc! { "$match": {
            "sandbox.channel_creation": { "$exists": false },
            "sandbox.channel_provisioning": { "$exists": false },
            "sandbox.channel_home_provenance": { "$exists": false },
            "sandbox.channel_home_prepare_evidence": { "$exists": false },
            "sandbox.channel_retirement": { "$exists": false },
            "sandbox.channel_storage_retention": { "$exists": false },
        } },
        doc! { "$lookup": { "from": "bridge_allocations", "localField": "_id",
            "foreignField": "sandbox_admission.sandbox_id", "pipeline": [
                { "$limit": 1 }, { "$project": { "_id": 1 } },
            ], "as": "channel_admission" } },
        doc! { "$match": { "channel_admission": { "$size": 0 } } },
        doc! { "$lookup": { "from": "bridge_allocations", "localField": "_id",
            "foreignField": "record.placement.target.sandbox_id", "pipeline": [
                { "$match": super::super::channel_storage_retention::root_allocation_filter() },
                { "$limit": 1 }, { "$project": { "_id": 1 } },
            ], "as": "channel_allocation" } },
        doc! { "$match": { "channel_allocation": { "$size": 0 } } },
        doc! { "$sort": { "_id": 1 } },
        doc! { "$limit": PAGE_SIZE },
        doc! { "$project": { "_id": 1 } },
    ]).await.map_err(|_| unavailable("deleted sandbox archive scan failed"))?
        .try_collect().await.map_err(|_| unavailable("deleted sandbox archive scan failed"))?;
    let mut retired = 0;
    for candidate in candidates {
        let id = candidate
            .get_str("_id")
            .map_err(|_| unavailable("deleted sandbox archive identity is corrupt"))?;
        let Some(document) = store
            .sandboxes
            .find_one(doc! { "_id": id })
            .await
            .map_err(|_| unavailable("deleted sandbox archive authority is unavailable"))?
        else {
            continue;
        };
        if super::super::channel_storage_retention::required(
            &store.client.database(&store.sandboxes.namespace().db),
            id,
            Some(&document),
            None,
        )
        .await
        .map_err(|_| unavailable("Channel checkpoint retention authority is unavailable"))?
        {
            continue;
        }
        let sandbox = super::super::sandboxes::read_sandbox(&document)
            .map_err(|_| unavailable("deleted sandbox archive authority is corrupt"))?;
        // Retirement is irreversible. Never infer it from an orphaned archive or
        // a missing runtime, and do not shorten retained conversation history.
        if sandbox.retirement_archive_pending()
            || sandbox.id.as_str() != id
            || sandbox.status != SandboxStatus::Retired
            || sandbox.runtime.is_some()
            || sandbox.lifecycle.runtime_provision.is_some()
            || sandbox.lifecycle.operation.is_some()
            || sandbox.lifecycle.checkpoint_capture.is_some()
        {
            continue;
        }
        retire_archives(store, id, now).await?;
        retired += 1;
    }
    Ok(retired)
}

async fn channel_retained(
    store: &MongoCheckpointObjectStore,
    sandbox_id: &str,
) -> AdapterResult<bool> {
    let sandbox = store
        .sandboxes
        .find_one(doc! { "_id": sandbox_id })
        .await
        .map_err(|_| unavailable("Channel checkpoint retention authority is unavailable"))?;
    if let Some(document) = &sandbox {
        let parent = super::super::sandboxes::read_sandbox(document)
            .map_err(|_| unavailable("retirement archive authority is corrupt"))?;
        if parent.retirement_archive_pending() {
            return Ok(true);
        }
    }
    super::super::channel_storage_retention::required(
        &store.client.database(&store.sandboxes.namespace().db),
        sandbox_id,
        sandbox.as_ref(),
        None,
    )
    .await
    .map_err(|_| unavailable("Channel checkpoint retention authority is unavailable"))
}

async fn retire_archives(
    store: &MongoCheckpointObjectStore,
    sandbox_id: &str,
    now: DateTime,
) -> AdapterResult<()> {
    super::model::validate_sandbox_id(sandbox_id)?;
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
            doc! { "$set": { "state": "deleting", "cleanup_after": now } },
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
    Ok(())
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
    if channel_retained(store, &manifest.sandbox_id).await? {
        return Ok(false);
    }
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
    let permanently_deleted = store.objects.finalize_delete(&manifest.object_key).await?;
    if !manifest.upload_confirmed && !permanently_deleted {
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
