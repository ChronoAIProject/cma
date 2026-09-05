use layer2_core::sandbox::SandboxCheckpointRef;
use mongodb::bson::{doc, Bson};
use mongodb::gridfs::FilesCollectionDocument;
use mongodb::ClientSession;

use super::model::{manifest_id, read_manifest, validate_gridfs_file, ManifestDoc};
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
    let document = store
        .manifests
        .find_one(doc! {
            "_id": manifest_id(sandbox_id, &checkpoint.object_reference),
        })
        .session(&mut *session)
        .await
        .map_err(RetentionError::Driver)?;
    let Some(document) = document else {
        return Ok(());
    };
    let manifest = read_manifest(document)
        .map_err(|_| RetentionError::Corrupt("evicted checkpoint archive manifest is corrupt"))?;
    validate_identity(&manifest, sandbox_id, checkpoint)?;
    let file = store
        .files
        .find_one(doc! { "_id": manifest.gridfs_id })
        .session(&mut *session)
        .await
        .map_err(RetentionError::Driver)?
        .ok_or(RetentionError::Corrupt(
            "evicted checkpoint archive file is missing",
        ))?;
    let file: FilesCollectionDocument =
        mongodb::bson::deserialize_from_document(file).map_err(|_| {
            RetentionError::Corrupt("evicted checkpoint archive file metadata is corrupt")
        })?;
    validate_gridfs_file(&file, &manifest).map_err(|_| {
        RetentionError::Corrupt("evicted checkpoint archive file metadata is corrupt")
    })?;

    let gridfs_id = Bson::ObjectId(manifest.gridfs_id);
    store
        .chunks
        .delete_many(doc! { "files_id": &gridfs_id })
        .session(&mut *session)
        .await
        .map_err(RetentionError::Driver)?;
    let file = store
        .files
        .delete_one(doc! {
            "_id": &gridfs_id,
            "metadata.manifest_id": &manifest.id,
            "metadata.sandbox_id": &manifest.sandbox_id,
            "metadata.gridfs_id": manifest.gridfs_id,
        })
        .session(&mut *session)
        .await
        .map_err(RetentionError::Driver)?;
    if file.deleted_count != 1 {
        return Err(RetentionError::Corrupt(
            "evicted checkpoint archive file changed during retention",
        ));
    }
    let authority = store
        .manifests
        .delete_one(doc! {
            "_id": &manifest.id,
            "sandbox_id": &manifest.sandbox_id,
            "object_reference": &manifest.object_reference,
            "sha256": &manifest.sha256,
            "gridfs_id": manifest.gridfs_id,
        })
        .session(session)
        .await
        .map_err(RetentionError::Driver)?;
    if authority.deleted_count == 1 {
        Ok(())
    } else {
        Err(RetentionError::Corrupt(
            "evicted checkpoint archive manifest changed during retention",
        ))
    }
}

fn validate_identity(
    manifest: &ManifestDoc,
    sandbox_id: &str,
    checkpoint: &SandboxCheckpointRef,
) -> Result<(), RetentionError> {
    if manifest.sandbox_id == sandbox_id
        && manifest.object_reference == checkpoint.object_reference
        && manifest.sha256 == checkpoint.sha256
    {
        Ok(())
    } else {
        Err(RetentionError::Corrupt(
            "evicted checkpoint archive identity is corrupt",
        ))
    }
}
