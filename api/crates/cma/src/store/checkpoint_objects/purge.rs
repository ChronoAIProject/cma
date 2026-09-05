use futures_util::TryStreamExt;
use layer3_adapter::Result as AdapterResult;
use mongodb::bson::{doc, Bson, Document};
use mongodb::gridfs::FilesCollectionDocument;

use super::model::{
    find_file, read_manifest, validate_gridfs_file, validate_manifest, GridFsMetadata, ManifestDoc,
};
use super::{unavailable, MongoCheckpointObjectStore};

const MAX_OBJECTS_PER_PURGE: usize = 64;
const PURGE_RETRY: &str = "checkpoint archive purge has more work and must be retried";

pub(super) async fn purge_sandbox(
    store: &MongoCheckpointObjectStore,
    sandbox_id: &str,
) -> AdapterResult<()> {
    let documents: Vec<Document> = store
        .manifests
        .find(doc! { "sandbox_id": sandbox_id })
        .sort(doc! { "_id": 1 })
        .limit(MAX_OBJECTS_PER_PURGE as i64)
        .await
        .map_err(|_| unavailable("checkpoint archive purge manifest scan failed"))?
        .try_collect()
        .await
        .map_err(|_| unavailable("checkpoint archive purge manifest scan failed"))?;

    let mut completed = 0_usize;
    for document in documents {
        let manifest = read_manifest(document)?;
        if manifest.sandbox_id != sandbox_id {
            return Err(unavailable("checkpoint archive purge ownership is corrupt"));
        }
        delete_authority_object(store, &manifest).await?;
        completed += 1;
    }

    if has_manifest(store, sandbox_id).await? {
        return Err(unavailable(PURGE_RETRY));
    }

    let remaining = MAX_OBJECTS_PER_PURGE.saturating_sub(completed);
    if remaining > 0 {
        purge_unpublished_files(store, sandbox_id, remaining).await?;
    }
    if has_file(store, sandbox_id).await? {
        Err(unavailable(PURGE_RETRY))
    } else {
        Ok(())
    }
}

async fn purge_unpublished_files(
    store: &MongoCheckpointObjectStore,
    sandbox_id: &str,
    limit: usize,
) -> AdapterResult<()> {
    let files: Vec<FilesCollectionDocument> = store
        .bucket
        .find(doc! { "metadata.sandbox_id": sandbox_id })
        .sort(doc! { "_id": 1 })
        .limit(limit as i64)
        .await
        .map_err(|_| unavailable("checkpoint archive purge file scan failed"))?
        .try_collect()
        .await
        .map_err(|_| unavailable("checkpoint archive purge file scan failed"))?;

    for file in files {
        let staged = staged_manifest(&file, sandbox_id)?;
        let authority = store
            .manifests
            .find_one(doc! { "gridfs_id": staged.gridfs_id })
            .await
            .map_err(|_| unavailable("checkpoint archive purge authority lookup failed"))?
            .map(read_manifest)
            .transpose()?;
        match authority {
            Some(authority) if authority == staged => {
                delete_authority_object(store, &authority).await?;
            }
            Some(_) => {
                return Err(unavailable("checkpoint archive purge ownership is corrupt"));
            }
            None => delete_file(store, &staged).await?,
        }
    }
    Ok(())
}

fn staged_manifest(file: &FilesCollectionDocument, sandbox_id: &str) -> AdapterResult<ManifestDoc> {
    let metadata = file
        .metadata
        .clone()
        .ok_or_else(|| unavailable("checkpoint archive purge file metadata is corrupt"))?;
    let metadata: GridFsMetadata = mongodb::bson::deserialize_from_document(metadata)
        .map_err(|_| unavailable("checkpoint archive purge file metadata is corrupt"))?;
    let manifest = metadata.manifest();
    validate_manifest(&manifest)?;
    validate_gridfs_file(file, &manifest)?;
    if manifest.sandbox_id != sandbox_id {
        return Err(unavailable("checkpoint archive purge ownership is corrupt"));
    }
    Ok(manifest)
}

async fn delete_authority_object(
    store: &MongoCheckpointObjectStore,
    manifest: &ManifestDoc,
) -> AdapterResult<()> {
    delete_file(store, manifest).await?;
    let result = store
        .manifests
        .delete_one(doc! {
            "_id": &manifest.id,
            "sandbox_id": &manifest.sandbox_id,
            "gridfs_id": manifest.gridfs_id,
        })
        .await
        .map_err(|_| unavailable("checkpoint archive purge manifest delete failed"))?;
    if result.deleted_count == 1 {
        return Ok(());
    }
    let remains = store
        .manifests
        .find_one(doc! { "_id": &manifest.id })
        .projection(doc! { "_id": 1 })
        .await
        .map_err(|_| unavailable("checkpoint archive purge reconciliation failed"))?;
    if remains.is_none() {
        Ok(())
    } else {
        Err(unavailable(
            "checkpoint archive purge manifest changed during deletion",
        ))
    }
}

pub(super) async fn delete_unpublished_file(
    store: &MongoCheckpointObjectStore,
    staged: &ManifestDoc,
) -> AdapterResult<()> {
    delete_file(store, staged).await
}

async fn delete_file(
    store: &MongoCheckpointObjectStore,
    manifest: &ManifestDoc,
) -> AdapterResult<()> {
    let existing = find_file(store, manifest.gridfs_id).await?;
    if let Some(file) = &existing {
        validate_gridfs_file(file, manifest)?;
    }
    let id = Bson::ObjectId(manifest.gridfs_id);
    store
        .chunks
        .delete_many(doc! { "files_id": &id })
        .await
        .map_err(|_| unavailable("checkpoint archive purge chunk delete failed"))?;
    let result = store
        .files
        .delete_one(doc! {
            "_id": &id,
            "metadata.manifest_id": &manifest.id,
            "metadata.sandbox_id": &manifest.sandbox_id,
            "metadata.gridfs_id": manifest.gridfs_id,
        })
        .await
        .map_err(|_| unavailable("checkpoint archive purge file delete failed"))?;
    if result.deleted_count == 1 {
        return Ok(());
    }
    let remains = store
        .files
        .find_one(doc! { "_id": &id })
        .projection(doc! { "_id": 1 })
        .await
        .map_err(|_| unavailable("checkpoint archive purge reconciliation failed"))?;
    if remains.is_none() {
        Ok(())
    } else {
        Err(unavailable(
            "checkpoint archive purge file changed during deletion",
        ))
    }
}

async fn has_manifest(store: &MongoCheckpointObjectStore, sandbox_id: &str) -> AdapterResult<bool> {
    store
        .manifests
        .find_one(doc! { "sandbox_id": sandbox_id })
        .projection(doc! { "_id": 1 })
        .await
        .map(|document| document.is_some())
        .map_err(|_| unavailable("checkpoint archive purge reconciliation failed"))
}

async fn has_file(store: &MongoCheckpointObjectStore, sandbox_id: &str) -> AdapterResult<bool> {
    store
        .files
        .find_one(doc! { "metadata.sandbox_id": sandbox_id })
        .projection(doc! { "_id": 1 })
        .await
        .map(|document| document.is_some())
        .map_err(|_| unavailable("checkpoint archive purge reconciliation failed"))
}
