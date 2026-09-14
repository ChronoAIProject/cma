use layer3_adapter::hosted::CheckpointArchiveDownload;

use super::model::{find_manifest, validate_reference};
use super::{rejected, unavailable, AdapterResult, MongoCheckpointObjectStore};

pub(super) async fn download(
    store: &MongoCheckpointObjectStore,
    sandbox_id: &str,
    object_reference: &str,
) -> AdapterResult<CheckpointArchiveDownload> {
    validate_reference(object_reference)?;
    let manifest = find_manifest(store, sandbox_id, object_reference)
        .await?
        .ok_or_else(|| rejected("checkpoint archive was not found"))?;
    if manifest.sandbox_id != sandbox_id || manifest.object_reference != object_reference {
        return Err(unavailable(
            "checkpoint archive manifest ownership is corrupt",
        ));
    }
    store
        .objects
        .download(
            &manifest.object_key,
            object_reference,
            &manifest.sha256,
            manifest.size_bytes as u64,
        )
        .await
}
