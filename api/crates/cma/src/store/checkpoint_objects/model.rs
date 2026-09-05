use layer2_core::StoreError;
use layer3_adapter::hosted::{CheckpointArchiveReceipt, MAX_CHECKPOINT_ARCHIVE_BYTES};
use layer3_adapter::Result as AdapterResult;
use mongodb::bson::{doc, oid::ObjectId, Bson, Document};
use mongodb::gridfs::FilesCollectionDocument;
use mongodb::options::{GridFsBucketOptions, IndexOptions, ReadConcern, WriteConcern};
use mongodb::{Collection, IndexModel};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{rejected, unavailable, MongoCheckpointObjectStore};
use crate::store::schema::DOCUMENT_SCHEMA_VERSION;

pub(super) const MANIFESTS: &str = "checkpoint_objects";
pub(super) const GRIDFS_BUCKET: &str = "checkpoint_archive_blobs";
pub(super) const GRIDFS_FILES: &str = "checkpoint_archive_blobs.files";
pub(super) const GRIDFS_CHUNKS: &str = "checkpoint_archive_blobs.chunks";
pub(super) const GRIDFS_CHUNK_BYTES: u32 = 1024 * 1024;
pub(super) const STREAM_CHUNK_BYTES: usize = 256 * 1024;
pub(super) const STREAM_CHANNEL_CAPACITY: usize = 2;

const MAX_SANDBOX_ID_BYTES: usize = 128;
const MANIFEST_ID_DOMAIN: &[u8] = b"cma-checkpoint-object-manifest-v1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ArchiveIdentity {
    pub object_reference: String,
    pub sha256: String,
    pub size_bytes: u64,
}

impl ArchiveIdentity {
    pub(super) fn from_input(
        object_reference: String,
        sha256: String,
        size_bytes: u64,
    ) -> AdapterResult<Self> {
        if !archive_fields_are_valid(&object_reference, &sha256, size_bytes) {
            return Err(rejected("checkpoint archive metadata is invalid"));
        }
        Ok(Self {
            object_reference,
            sha256,
            size_bytes,
        })
    }

    pub(super) fn receipt(&self) -> CheckpointArchiveReceipt {
        CheckpointArchiveReceipt {
            object_reference: self.object_reference.clone(),
            sha256: self.sha256.clone(),
            size_bytes: self.size_bytes,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManifestDoc {
    #[serde(rename = "_id")]
    pub id: String,
    pub document_schema_version: i64,
    pub sandbox_id: String,
    pub object_reference: String,
    pub sha256: String,
    pub size_bytes: i64,
    pub gridfs_id: ObjectId,
    pub chunk_size_bytes: i64,
}

impl ManifestDoc {
    pub(super) fn new(sandbox_id: &str, identity: &ArchiveIdentity, gridfs_id: ObjectId) -> Self {
        Self {
            id: manifest_id(sandbox_id, &identity.object_reference),
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            sandbox_id: sandbox_id.to_owned(),
            object_reference: identity.object_reference.clone(),
            sha256: identity.sha256.clone(),
            size_bytes: identity.size_bytes as i64,
            gridfs_id,
            chunk_size_bytes: i64::from(GRIDFS_CHUNK_BYTES),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GridFsMetadata {
    pub document_schema_version: i64,
    pub manifest_id: String,
    pub sandbox_id: String,
    pub object_reference: String,
    pub sha256: String,
    pub size_bytes: i64,
    pub gridfs_id: ObjectId,
    pub chunk_size_bytes: i64,
}

impl From<&ManifestDoc> for GridFsMetadata {
    fn from(manifest: &ManifestDoc) -> Self {
        Self {
            document_schema_version: manifest.document_schema_version,
            manifest_id: manifest.id.clone(),
            sandbox_id: manifest.sandbox_id.clone(),
            object_reference: manifest.object_reference.clone(),
            sha256: manifest.sha256.clone(),
            size_bytes: manifest.size_bytes,
            gridfs_id: manifest.gridfs_id,
            chunk_size_bytes: manifest.chunk_size_bytes,
        }
    }
}

impl GridFsMetadata {
    pub(super) fn manifest(&self) -> ManifestDoc {
        ManifestDoc {
            id: self.manifest_id.clone(),
            document_schema_version: self.document_schema_version,
            sandbox_id: self.sandbox_id.clone(),
            object_reference: self.object_reference.clone(),
            sha256: self.sha256.clone(),
            size_bytes: self.size_bytes,
            gridfs_id: self.gridfs_id,
            chunk_size_bytes: self.chunk_size_bytes,
        }
    }
}

pub(super) fn validate_sandbox_id(sandbox_id: &str) -> AdapterResult<()> {
    if sandbox_id_is_valid(sandbox_id) {
        Ok(())
    } else {
        Err(rejected("checkpoint archive sandbox identity is invalid"))
    }
}

pub(super) fn validate_reference(object_reference: &str) -> AdapterResult<()> {
    if reference_digest(object_reference).is_some() {
        Ok(())
    } else {
        Err(rejected("checkpoint archive reference is invalid"))
    }
}

fn sandbox_id_is_valid(sandbox_id: &str) -> bool {
    sandbox_id.len() > "sbx_".len()
        && sandbox_id.len() <= MAX_SANDBOX_ID_BYTES
        && sandbox_id.starts_with("sbx_")
        && sandbox_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn archive_fields_are_valid(object_reference: &str, sha256: &str, size_bytes: u64) -> bool {
    reference_digest(object_reference) == Some(sha256)
        && lowercase_sha256(sha256)
        && (1..=MAX_CHECKPOINT_ARCHIVE_BYTES).contains(&size_bytes)
}

fn reference_digest(object_reference: &str) -> Option<&str> {
    let digest = object_reference
        .strip_prefix("home-")?
        .strip_suffix(".cma")?;
    lowercase_sha256(digest).then_some(digest)
}

fn lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn manifest_id(sandbox_id: &str, object_reference: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(MANIFEST_ID_DOMAIN);
    for part in [sandbox_id.as_bytes(), object_reference.as_bytes()] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part);
    }
    format!("cpo_{:x}", digest.finalize())
}

#[cfg(test)]
pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn finish_sha256(digest: Sha256) -> String {
    format!("{:x}", digest.finalize())
}

pub(super) fn manifest_document(manifest: &ManifestDoc) -> AdapterResult<Document> {
    mongodb::bson::serialize_to_document(manifest)
        .map_err(|_| unavailable("checkpoint archive manifest could not be encoded"))
}

pub(super) fn metadata_document(manifest: &ManifestDoc) -> AdapterResult<Document> {
    mongodb::bson::serialize_to_document(&GridFsMetadata::from(manifest))
        .map_err(|_| unavailable("checkpoint archive file metadata could not be encoded"))
}

pub(super) fn read_manifest(document: Document) -> AdapterResult<ManifestDoc> {
    let manifest: ManifestDoc = mongodb::bson::deserialize_from_document(document)
        .map_err(|_| unavailable("checkpoint archive manifest is corrupt"))?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

pub(super) fn validate_manifest(manifest: &ManifestDoc) -> AdapterResult<()> {
    let size = u64::try_from(manifest.size_bytes).ok();
    let valid = manifest.document_schema_version == DOCUMENT_SCHEMA_VERSION
        && sandbox_id_is_valid(&manifest.sandbox_id)
        && size.is_some_and(|size| {
            archive_fields_are_valid(&manifest.object_reference, &manifest.sha256, size)
        })
        && manifest.chunk_size_bytes == i64::from(GRIDFS_CHUNK_BYTES)
        && manifest.id == manifest_id(&manifest.sandbox_id, &manifest.object_reference);
    if valid {
        Ok(())
    } else {
        Err(unavailable("checkpoint archive manifest is corrupt"))
    }
}

pub(super) fn manifest_matches(
    manifest: &ManifestDoc,
    sandbox_id: &str,
    identity: &ArchiveIdentity,
) -> bool {
    manifest.sandbox_id == sandbox_id
        && manifest.object_reference == identity.object_reference
        && manifest.sha256 == identity.sha256
        && manifest.size_bytes == identity.size_bytes as i64
}

pub(super) async fn find_manifest(
    store: &MongoCheckpointObjectStore,
    sandbox_id: &str,
    object_reference: &str,
) -> AdapterResult<Option<ManifestDoc>> {
    let id = manifest_id(sandbox_id, object_reference);
    let by_id = store
        .manifests
        .find_one(doc! { "_id": &id })
        .await
        .map_err(|_| unavailable("checkpoint archive manifest lookup failed"))?;
    if let Some(document) = by_id {
        return read_manifest(document).map(Some);
    }

    let by_identity = store
        .manifests
        .find_one(doc! {
            "sandbox_id": sandbox_id,
            "object_reference": object_reference,
        })
        .await
        .map_err(|_| unavailable("checkpoint archive manifest lookup failed"))?;
    by_identity.map(read_manifest).transpose()
}

pub(super) async fn find_validated_file(
    store: &MongoCheckpointObjectStore,
    manifest: &ManifestDoc,
) -> AdapterResult<FilesCollectionDocument> {
    let file = find_file(store, manifest.gridfs_id)
        .await?
        .ok_or_else(|| unavailable("checkpoint archive file is missing"))?;
    validate_gridfs_file(&file, manifest)?;
    Ok(file)
}

pub(super) async fn find_file(
    store: &MongoCheckpointObjectStore,
    gridfs_id: ObjectId,
) -> AdapterResult<Option<FilesCollectionDocument>> {
    store
        .bucket
        .find_one(doc! { "_id": Bson::ObjectId(gridfs_id) })
        .await
        .map_err(|_| unavailable("checkpoint archive file lookup failed"))
}

pub(super) fn validate_gridfs_file(
    file: &FilesCollectionDocument,
    manifest: &ManifestDoc,
) -> AdapterResult<GridFsMetadata> {
    let metadata = file
        .metadata
        .clone()
        .ok_or_else(|| unavailable("checkpoint archive file metadata is corrupt"))?;
    let metadata: GridFsMetadata = mongodb::bson::deserialize_from_document(metadata)
        .map_err(|_| unavailable("checkpoint archive file metadata is corrupt"))?;
    let expected = GridFsMetadata::from(manifest);
    let valid = file.id == Bson::ObjectId(manifest.gridfs_id)
        && file.filename.as_deref() == Some(manifest.object_reference.as_str())
        && file.length == manifest.size_bytes as u64
        && file.chunk_size_bytes == GRIDFS_CHUNK_BYTES
        && metadata == expected;
    if valid {
        Ok(metadata)
    } else {
        Err(unavailable("checkpoint archive file metadata is corrupt"))
    }
}

pub(super) fn bucket_options() -> GridFsBucketOptions {
    GridFsBucketOptions::builder()
        .bucket_name(GRIDFS_BUCKET.to_owned())
        .chunk_size_bytes(GRIDFS_CHUNK_BYTES)
        .read_concern(ReadConcern::majority())
        .write_concern(WriteConcern::majority())
        .build()
}

pub(super) async fn ensure_indexes(
    manifests: &Collection<Document>,
    files: &Collection<Document>,
    chunks: &Collection<Document>,
) -> Result<(), StoreError> {
    manifests
        .create_indexes(manifest_indexes())
        .await
        .map_err(|_| StoreError::Unavailable("checkpoint archive indexes are not usable".into()))?;
    files
        .create_indexes(file_indexes())
        .await
        .map_err(|_| StoreError::Unavailable("checkpoint archive indexes are not usable".into()))?;
    chunks
        .create_indexes(chunk_indexes())
        .await
        .map_err(|_| StoreError::Unavailable("checkpoint archive indexes are not usable".into()))?;
    Ok(())
}

pub(super) fn manifest_indexes() -> Vec<IndexModel> {
    vec![
        index(
            doc! { "sandbox_id": 1, "object_reference": 1 },
            "checkpoint_objects_sandbox_reference",
            true,
        ),
        index(
            doc! { "gridfs_id": 1 },
            "checkpoint_objects_gridfs_id",
            true,
        ),
        index(
            doc! { "sandbox_id": 1, "_id": 1 },
            "checkpoint_objects_sandbox_purge",
            false,
        ),
    ]
}

pub(super) fn file_indexes() -> Vec<IndexModel> {
    vec![
        IndexModel::builder()
            .keys(doc! { "filename": 1, "uploadDate": 1 })
            .build(),
        index(
            doc! { "metadata.sandbox_id": 1, "_id": 1 },
            "checkpoint_archive_files_sandbox_purge",
            false,
        ),
    ]
}

pub(super) fn chunk_indexes() -> Vec<IndexModel> {
    vec![IndexModel::builder()
        .keys(doc! { "files_id": 1, "n": 1 })
        .options(IndexOptions::builder().unique(true).build())
        .build()]
}

fn index(keys: Document, name: &str, unique: bool) -> IndexModel {
    IndexModel::builder()
        .keys(keys)
        .options(
            IndexOptions::builder()
                .name(name.to_owned())
                .unique(unique)
                .build(),
        )
        .build()
}
