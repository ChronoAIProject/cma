use std::time::SystemTime;

use layer3_adapter::hosted::{CheckpointArchiveReceipt, CheckpointArchiveUpload};
use mongodb::bson::DateTime;
use sha2::{Digest, Sha256};

use super::model::{find_manifest, ArchiveIdentity, ManifestDoc};
use super::{publication, rejected, unavailable, AdapterResult, MongoCheckpointObjectStore};
use crate::store::CheckpointBlobStore;

pub(super) async fn upload(
    store: &MongoCheckpointObjectStore,
    sandbox_id: &str,
    mut archive: CheckpointArchiveUpload,
) -> AdapterResult<CheckpointArchiveReceipt> {
    let identity = ArchiveIdentity::from_input(
        archive.object_reference.clone(),
        archive.sha256.clone(),
        archive.size_bytes,
    )?;
    if let Some(existing) = find_manifest(store, sandbox_id, &identity.object_reference).await? {
        publication::verify_active(store, sandbox_id).await?;
        if !existing.matches(sandbox_id, &identity) {
            return Err(rejected(
                "checkpoint archive reference has conflicting metadata",
            ));
        }
        let mut size = 0_u64;
        let mut digest = Sha256::new();
        while let Some(chunk) = archive.chunks.recv().await {
            let chunk = chunk?;
            size = size
                .checked_add(chunk.len() as u64)
                .filter(|size| *size <= identity.size_bytes)
                .ok_or_else(|| rejected("checkpoint archive replay exceeds its declared size"))?;
            digest.update(&chunk);
        }
        if size != identity.size_bytes || format!("{:x}", digest.finalize()) != identity.sha256 {
            return Err(rejected(
                "checkpoint archive replay failed integrity verification",
            ));
        }
        let mut retained = store
            .objects
            .download(
                &existing.object_key,
                &identity.object_reference,
                &identity.sha256,
                identity.size_bytes,
            )
            .await?
            .chunks;
        while let Some(chunk) = retained.recv().await {
            chunk?;
        }
        publication::verify_active(store, sandbox_id).await?;
        return Ok(identity.receipt());
    }
    let staged = ManifestDoc::new(sandbox_id, &identity, DateTime::now());
    publication::admit(store, &staged).await?;
    transfer_before_deadline(store.objects.as_ref(), &staged, archive, SystemTime::now()).await?;
    publication::publish(store, &staged, &identity).await?;
    Ok(identity.receipt())
}

async fn transfer_before_deadline(
    objects: &dyn CheckpointBlobStore,
    staged: &ManifestDoc,
    archive: CheckpointArchiveUpload,
    now: SystemTime,
) -> AdapterResult<()> {
    let remaining = staged
        .cleanup_after
        .to_system_time()
        .duration_since(now)
        .map_err(|_| unavailable("checkpoint archive upload lifetime expired"))?;
    tokio::time::timeout(remaining, objects.upload(&staged.object_key, archive))
        .await
        .map_err(|_| unavailable("checkpoint archive upload lifetime expired"))?
}

#[cfg(test)]
#[path = "upload_tests.rs"]
mod tests;
