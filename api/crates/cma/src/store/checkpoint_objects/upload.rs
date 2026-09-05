use futures_util::io::AsyncWriteExt;
use layer3_adapter::hosted::{
    CheckpointArchiveReceipt, CheckpointArchiveUpload, MAX_CHECKPOINT_ARCHIVE_BYTES,
};
use layer3_adapter::Result as AdapterResult;
use mongodb::bson::{oid::ObjectId, Bson};
use mongodb::gridfs::GridFsUploadStream;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use super::model::{
    find_manifest, find_validated_file, finish_sha256, manifest_document, manifest_matches,
    metadata_document, ArchiveIdentity, ManifestDoc, GRIDFS_CHUNK_BYTES,
};
use super::{purge, rejected, unavailable, MongoCheckpointObjectStore};

pub(super) async fn upload(
    store: &MongoCheckpointObjectStore,
    sandbox_id: &str,
    archive: CheckpointArchiveUpload,
) -> AdapterResult<CheckpointArchiveReceipt> {
    let CheckpointArchiveUpload {
        object_reference,
        sha256,
        size_bytes,
        mut chunks,
    } = archive;
    let identity = ArchiveIdentity::from_input(object_reference, sha256, size_bytes)?;

    if let Some(existing) = find_manifest(store, sandbox_id, &identity.object_reference).await? {
        if !manifest_matches(&existing, sandbox_id, &identity) {
            return Err(rejected(
                "checkpoint archive reference is already committed with different metadata",
            ));
        }
        find_validated_file(store, &existing).await?;
        verify_replay(&mut chunks, &identity).await?;
        return Ok(identity.receipt());
    }

    stage_and_publish(store, sandbox_id, identity, chunks).await
}

async fn verify_replay(
    chunks: &mut mpsc::Receiver<AdapterResult<Vec<u8>>>,
    identity: &ArchiveIdentity,
) -> AdapterResult<()> {
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    while let Some(chunk) = chunks.recv().await {
        let chunk = chunk.map_err(|_| unavailable("checkpoint archive upload stream failed"))?;
        size = checked_size(size, chunk.len(), identity.size_bytes)?;
        digest.update(&chunk);
    }
    verify_seal(size, digest, identity)
}

async fn stage_and_publish(
    store: &MongoCheckpointObjectStore,
    sandbox_id: &str,
    identity: ArchiveIdentity,
    chunks: mpsc::Receiver<AdapterResult<Vec<u8>>>,
) -> AdapterResult<CheckpointArchiveReceipt> {
    let gridfs_id = ObjectId::new();
    let manifest = ManifestDoc::new(sandbox_id, &identity, gridfs_id);
    let metadata = metadata_document(&manifest)?;
    let mut stream = store
        .bucket
        .open_upload_stream(&identity.object_reference)
        .id(Bson::ObjectId(gridfs_id))
        .chunk_size_bytes(GRIDFS_CHUNK_BYTES)
        .metadata(metadata)
        .await
        .map_err(|_| unavailable("checkpoint archive upload could not start"))?;

    if let Err(error) = stream_archive(chunks, &identity, &mut stream).await {
        return match error {
            StreamFailure::Abort(error) => Err(abort_or_cleanup_error(&mut stream, error).await),
            StreamFailure::Driver(error) => Err(error),
        };
    }
    stream
        .close()
        .await
        .map_err(|_| unavailable("checkpoint archive upload could not be finalized"))?;
    find_validated_file(store, &manifest).await?;

    // GridFS cannot share a streaming transaction with this publication marker.
    // A crash in this gap leaves only an unreachable file that sandbox purge reclaims.
    let document = manifest_document(&manifest)?;
    match store.manifests.insert_one(document).await {
        Ok(_) => Ok(identity.receipt()),
        Err(_) => reconcile_publication(store, sandbox_id, &identity, &manifest).await,
    }
}

async fn stream_archive(
    mut chunks: mpsc::Receiver<AdapterResult<Vec<u8>>>,
    identity: &ArchiveIdentity,
    stream: &mut GridFsUploadStream,
) -> Result<(), StreamFailure> {
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    while let Some(chunk) = chunks.recv().await {
        let chunk = chunk.map_err(|_| {
            StreamFailure::Abort(unavailable("checkpoint archive upload stream failed"))
        })?;
        size =
            checked_size(size, chunk.len(), identity.size_bytes).map_err(StreamFailure::Abort)?;
        digest.update(&chunk);
        for part in chunk.chunks(GRIDFS_CHUNK_BYTES as usize) {
            stream.write_all(part).await.map_err(|_| {
                StreamFailure::Driver(unavailable("checkpoint archive upload write failed"))
            })?;
        }
    }
    verify_seal(size, digest, identity).map_err(StreamFailure::Abort)
}

enum StreamFailure {
    Abort(layer3_adapter::AdapterError),
    Driver(layer3_adapter::AdapterError),
}

fn checked_size(current: u64, chunk_len: usize, expected: u64) -> AdapterResult<u64> {
    let chunk_len = u64::try_from(chunk_len)
        .map_err(|_| rejected("checkpoint archive exceeds its declared size"))?;
    let size = current
        .checked_add(chunk_len)
        .ok_or_else(|| rejected("checkpoint archive exceeds its declared size"))?;
    if size > expected || size > MAX_CHECKPOINT_ARCHIVE_BYTES {
        Err(rejected("checkpoint archive exceeds its declared size"))
    } else {
        Ok(size)
    }
}

fn verify_seal(size: u64, digest: Sha256, identity: &ArchiveIdentity) -> AdapterResult<()> {
    if size != identity.size_bytes {
        return Err(rejected(
            "checkpoint archive is shorter than its declared size",
        ));
    }
    if finish_sha256(digest) != identity.sha256 {
        return Err(rejected(
            "checkpoint archive does not match its declared SHA-256",
        ));
    }
    Ok(())
}

async fn abort_or_cleanup_error(
    stream: &mut GridFsUploadStream,
    original: layer3_adapter::AdapterError,
) -> layer3_adapter::AdapterError {
    if stream.abort().await.is_ok() {
        original
    } else {
        unavailable("checkpoint archive upload cleanup failed")
    }
}

async fn reconcile_publication(
    store: &MongoCheckpointObjectStore,
    sandbox_id: &str,
    identity: &ArchiveIdentity,
    staged: &ManifestDoc,
) -> AdapterResult<CheckpointArchiveReceipt> {
    let reconciliation = find_manifest(store, sandbox_id, &identity.object_reference).await;
    let existing = match reconciliation {
        Ok(existing) => existing,
        Err(error) => {
            purge::delete_unpublished_file(store, staged).await?;
            return Err(error);
        }
    };

    let Some(existing) = existing else {
        purge::delete_unpublished_file(store, staged).await?;
        return Err(unavailable(
            "checkpoint archive publication could not be reconciled",
        ));
    };
    if !manifest_matches(&existing, sandbox_id, identity) {
        purge::delete_unpublished_file(store, staged).await?;
        return Err(rejected(
            "checkpoint archive reference is already committed with different metadata",
        ));
    }
    if let Err(error) = find_validated_file(store, &existing).await {
        if existing.gridfs_id != staged.gridfs_id {
            purge::delete_unpublished_file(store, staged).await?;
        }
        return Err(error);
    }
    if existing.gridfs_id != staged.gridfs_id {
        purge::delete_unpublished_file(store, staged).await?;
    }
    Ok(identity.receipt())
}
