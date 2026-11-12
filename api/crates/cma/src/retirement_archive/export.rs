use super::*;
use crate::store::retirement_archive::{capture, schema_sha256, ArchiveStore};
use layer2_core::document_artifacts::DocumentBlobStore;
use layer3_adapter::{
    archive_source::{Kind, SnapshotTree},
    permanent_custody::{digest, ObjectReceipt, PermanentCustody},
};
use sha2::{Digest, Sha256};

pub(crate) fn exclusion(request: &Request) -> Result<Vec<u8>> {
    let bytes =
        layer3_adapter::archive_source::read_private_file(&request.exclusion_receipt, 1024 * 1024)?;
    if digest(&bytes) != request.exclusion_sha256 {
        return Err(REFUSED);
    }
    let value: ExclusionReceipt = serde_json::from_slice(&bytes).map_err(|_| REFUSED)?;
    value.require(request, &layer2_core::now_rfc3339())?;
    Ok(bytes)
}
pub(crate) async fn build(
    store: &ArchiveStore,
    request: &Request,
    bytes: &[u8],
    custody: &dyn PermanentCustody,
    blobs: Option<&dyn DocumentBlobStore>,
) -> Result<Manifest> {
    request.validate()?;
    let exclusion = exclusion(request)?;
    let policy = custody.policy().await?;
    let original = store.original(request, bytes).await?;
    let mut files = Vec::new();
    let mut source_bytes = 0u64;
    for root in &request.roots {
        let tree = SnapshotTree::open(&root.path)?;
        let metadata = &tree.entries.first().ok_or(REFUSED)?.metadata;
        if metadata.device != root.device || metadata.inode != root.inode {
            return Err(REFUSED);
        }
        for entry in &tree.entries {
            if files.len() >= 200_000 {
                return Err(REFUSED);
            }
            let mut objects = Vec::new();
            let mut hash = Sha256::new();
            if entry.kind == Kind::File {
                let mut reader = tree.reader(entry)?;
                while let Some(data) = reader.chunk()? {
                    source_bytes = source_bytes.checked_add(data.len() as u64).ok_or(REFUSED)?;
                    if source_bytes > 1024 * 1024 * 1024 * 1024 {
                        return Err(REFUSED);
                    }
                    hash.update(&data);
                    objects.push(custody.retain(&request.operation_id, data).await?);
                }
            }
            files.push(FileRecord {
                role: root.role.clone(),
                entry: entry.clone(),
                sha256: format!("{:x}", hash.finalize()),
                objects,
            });
        }
        tree.verify_unchanged()?;
    }
    let (database, external) = capture(store, request, bytes, custody, blobs).await?;
    let manifest = Manifest {
        format: 1,
        operation_id: request.operation_id.clone(),
        request_sha256: digest(bytes),
        original_request: custody
            .retain(&request.operation_id, bytes.to_vec())
            .await?,
        original_source: custody.retain(&request.operation_id, original).await?,
        exclusion: custody.retain(&request.operation_id, exclusion).await?,
        custody: policy.clone(),
        schema_sha256: schema_sha256(),
        external,
        files,
        database,
    };
    if custody.policy().await? != policy {
        return Err(REFUSED);
    }
    Ok(manifest)
}
pub(crate) async fn publish(
    store: &ArchiveStore,
    request: &Request,
    bytes: &[u8],
    custody: &dyn PermanentCustody,
    blobs: Option<&dyn DocumentBlobStore>,
) -> Result<ObjectReceipt> {
    let manifest = build(store, request, bytes, custody, blobs).await?;
    // A second independent source enumeration rejects drift even when a source
    // changed after its first descriptor closed. Physical fencing remains required.
    if build(store, request, bytes, custody, blobs).await? != manifest {
        return Err(REFUSED);
    }
    let receipt = custody
        .retain(
            &request.operation_id,
            serde_json::to_vec(&manifest).map_err(|_| REFUSED)?,
        )
        .await?;
    store.retain_manifest(request, bytes, &receipt).await?;
    Ok(receipt)
}
