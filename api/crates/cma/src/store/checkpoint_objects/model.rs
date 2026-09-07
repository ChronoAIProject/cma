use layer2_core::StoreError;
use layer3_adapter::hosted::{CheckpointArchiveReceipt, MAX_CHECKPOINT_ARCHIVE_BYTES};
use layer3_adapter::Result as AdapterResult;
use mongodb::bson::{doc, DateTime, Document};
use mongodb::options::IndexOptions;
use mongodb::{Collection, IndexModel};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{rejected, unavailable, MongoCheckpointObjectStore};
use crate::store::checkpoint_storage::UPLOAD_LIFETIME;
use crate::store::schema::DOCUMENT_SCHEMA_VERSION;

pub(super) const MANIFESTS: &str = "checkpoint_objects";
pub(super) const GATES: &str = "checkpoint_archive_gates";
pub(super) const TOMBSTONES: &str = "checkpoint_archive_tombstones";

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
        if reference_digest(&object_reference) != Some(sha256.as_str())
            || !(1..=MAX_CHECKPOINT_ARCHIVE_BYTES).contains(&size_bytes)
        {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ArchiveState {
    Uploading,
    Published,
    Deleting,
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
    pub object_key: String,
    pub state: ArchiveState,
    pub upload_confirmed: bool,
    pub cleanup_after: DateTime,
}

impl ManifestDoc {
    pub(super) fn new(sandbox_id: &str, identity: &ArchiveIdentity, now: DateTime) -> Self {
        let id = format!("cpa_{}", uuid::Uuid::new_v4().simple());
        Self {
            object_key: crate::store::checkpoint_storage::checkpoint_object_key(
                object_key(sandbox_id, &id),
                identity.size_bytes,
            ),
            id,
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            sandbox_id: sandbox_id.to_owned(),
            object_reference: identity.object_reference.clone(),
            sha256: identity.sha256.clone(),
            size_bytes: identity.size_bytes as i64,
            state: ArchiveState::Uploading,
            upload_confirmed: false,
            cleanup_after: DateTime::from_millis(
                now.timestamp_millis()
                    .saturating_add(UPLOAD_LIFETIME.as_millis() as i64),
            ),
        }
    }

    pub(super) fn matches(&self, sandbox_id: &str, identity: &ArchiveIdentity) -> bool {
        self.sandbox_id == sandbox_id
            && self.object_reference == identity.object_reference
            && self.sha256 == identity.sha256
            && self.size_bytes == identity.size_bytes as i64
    }
}

pub(super) fn validate_sandbox_id(sandbox_id: &str) -> AdapterResult<()> {
    if sandbox_id.len() > 4
        && sandbox_id.len() <= 128
        && sandbox_id.starts_with("sbx_")
        && sandbox_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        Ok(())
    } else {
        Err(rejected("checkpoint archive sandbox identity is invalid"))
    }
}

pub(super) fn validate_reference(reference: &str) -> AdapterResult<()> {
    reference_digest(reference)
        .map(|_| ())
        .ok_or_else(|| rejected("checkpoint archive reference is invalid"))
}

fn reference_digest(reference: &str) -> Option<&str> {
    let digest = reference.strip_prefix("home-")?.strip_suffix(".cma")?;
    (digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')))
    .then_some(digest)
}

fn object_key(sandbox_id: &str, id: &str) -> String {
    format!(
        "cma/checkpoints/{:x}/{id}.cma",
        Sha256::digest(sandbox_id.as_bytes())
    )
}

pub(super) fn manifest_document(manifest: &ManifestDoc) -> AdapterResult<Document> {
    mongodb::bson::serialize_to_document(manifest)
        .map_err(|_| unavailable("checkpoint archive metadata could not be encoded"))
}

pub(super) fn read_manifest(document: Document) -> AdapterResult<ManifestDoc> {
    let manifest: ManifestDoc = mongodb::bson::deserialize_from_document(document)
        .map_err(|_| unavailable("checkpoint archive metadata is corrupt"))?;
    let id = manifest.id.strip_prefix("cpa_");
    let valid = manifest.document_schema_version == DOCUMENT_SCHEMA_VERSION
        && validate_sandbox_id(&manifest.sandbox_id).is_ok()
        && id.is_some_and(|value| {
            value.len() == 32
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        })
        && (manifest.object_key == object_key(&manifest.sandbox_id, &manifest.id)
            || manifest.object_key
                == crate::store::checkpoint_storage::checkpoint_object_key(
                    object_key(&manifest.sandbox_id, &manifest.id),
                    manifest.size_bytes as u64,
                ))
        && manifest.cleanup_after.timestamp_millis() > 0
        && (manifest.state != ArchiveState::Published || manifest.upload_confirmed)
        && (manifest.state != ArchiveState::Uploading || !manifest.upload_confirmed)
        && u64::try_from(manifest.size_bytes).ok().is_some_and(|size| {
            ArchiveIdentity::from_input(
                manifest.object_reference.clone(),
                manifest.sha256.clone(),
                size,
            )
            .is_ok()
        });
    if valid {
        Ok(manifest)
    } else {
        Err(unavailable("checkpoint archive metadata is corrupt"))
    }
}

pub(super) async fn find_manifest(
    store: &MongoCheckpointObjectStore,
    sandbox_id: &str,
    object_reference: &str,
) -> AdapterResult<Option<ManifestDoc>> {
    store.manifests.find_one(doc! { "sandbox_id": sandbox_id, "object_reference": object_reference, "state": "published" })
        .await.map_err(|_| unavailable("checkpoint archive manifest lookup failed"))?.map(read_manifest).transpose()
}

pub(super) async fn ensure_indexes(manifests: &Collection<Document>) -> Result<(), StoreError> {
    manifests
        .create_indexes(manifest_indexes())
        .await
        .map_err(|_| StoreError::Unavailable("checkpoint archive indexes are not usable".into()))?;
    Ok(())
}

pub(super) fn manifest_indexes() -> Vec<IndexModel> {
    vec![
        IndexModel::builder()
            .keys(doc! { "sandbox_id": 1, "object_reference": 1 })
            .options(
                IndexOptions::builder()
                    .name("checkpoint_objects_published_identity".to_owned())
                    .unique(true)
                    .partial_filter_expression(doc! { "state": "published" })
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "state": 1, "cleanup_after": 1, "_id": 1 })
            .build(),
        IndexModel::builder()
            .keys(doc! { "sandbox_id": 1, "_id": 1 })
            .build(),
    ]
}

#[cfg(test)]
pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
