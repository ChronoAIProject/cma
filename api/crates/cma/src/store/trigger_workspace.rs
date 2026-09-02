//! Launch-owned repository baselines and checkpoints in platform storage.

use std::{collections::BTreeMap, sync::Arc};

use futures_util::StreamExt as _;
use layer2_core::{
    BaselineArchivePlan, BaselineImportBlob, BaselineImportChunk, BaselineImportOutcome,
    BaselineObjectStore, CheckpointIntegrity, CheckpointObjectStore, DiffArtifact, DiffArtifactId,
    DiffArtifactObjectStore, LaunchId, LaunchStore, LifecycleCheckpointState,
    SessionWorkspaceSource, StoreError, SubjectId, TriggerRecoveryPair,
    TriggerRecoveryPairCommitReceipt, TriggerRecoveryPairRef, TriggerSessionOrigin,
    TriggerStoreError, WorkspaceCheckpoint, WorkspaceCheckpointCommitOutcome,
    WorkspaceCheckpointFailure, WorkspaceCheckpointIntent, WorkspaceCheckpointPurpose,
    WorkspaceCheckpointReservation, WorkspaceEntry, WorkspaceEntryKind, WorkspaceManifest,
    BASELINE_IMPORT_CHUNK_BYTES, MAX_DIFF_ARTIFACT_BYTES, MAX_REPOSITORY_BYTES,
    MAX_REPOSITORY_FILES, MAX_TRIGGER_CONVERSATION_SNAPSHOT_BYTES,
};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::token::TokenSource;

const WORKSPACE_CONTENT_TYPE: &str = "application/vnd.chrono.cma-workspace.v1";
const RECOVERY_CONVERSATION_CONTENT_TYPE: &str =
    "application/vnd.chrono.cma-trigger-conversation.v1";
const IMPORT_INDEX_CONTENT_TYPE: &str = "application/vnd.chrono.cma-workspace-import.v1+json";
const IMPORT_CHUNK_CONTENT_TYPE: &str = "application/vnd.chrono.cma-workspace-chunk.v1";
const DIFF_CONTENT_TYPE: &str = "application/vnd.chrono.cma-trigger-diff.v1+json";
const ARCHIVE_OVERHEAD_BYTES: u64 = 256 * 1024 * 1024;
const IMPORT_INDEX_VERSION: u32 = 1;
const IMPORT_INDEX_MAX_BYTES: u64 = 256 * 1024 * 1024;
const IMPORT_RETIRED_MARKER: &[u8] = b"cma-baseline-import-chunks-retired-v1\n";
const RECOVERY_REQUIRED_CAS_RETRIES: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BaselineImportIndex {
    version: u32,
    launch_id: LaunchId,
    source_revision: String,
    tree_digest: String,
    blobs: Vec<BaselineImportBlob>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CompletedBaselineImport {
    version: u32,
    launch_id: LaunchId,
    source_revision: String,
    tree_digest: String,
    outcome: BaselineImportOutcome,
}

struct BaselineUploadStreamState {
    store: ChronoStorageTriggerWorkspaceStore,
    owner: SubjectId,
    launch: LaunchId,
    source_revision: String,
    preamble: Option<Vec<u8>>,
    chunks: std::vec::IntoIter<BaselineImportChunk>,
}

/// Repository artifacts reached through CMA's one storage machine identity.
#[derive(Clone)]
pub struct ChronoStorageTriggerWorkspaceStore {
    base_url: String,
    bucket: String,
    tokens: Arc<dyn TokenSource>,
    launches: Arc<dyn LaunchStore>,
    http: reqwest::Client,
}

impl ChronoStorageTriggerWorkspaceStore {
    pub fn new(
        base_url: impl Into<String>,
        bucket: impl Into<String>,
        tokens: Arc<dyn TokenSource>,
        launches: Arc<dyn LaunchStore>,
    ) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            bucket: bucket.into(),
            tokens,
            launches,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(180))
                .build()
                .expect("HTTP client builds"),
        }
    }

    fn objects_url(&self) -> String {
        format!("{}/api/buckets/{}/objects", self.base_url, self.bucket)
    }

    fn download_url(&self) -> String {
        format!("{}/download", self.objects_url())
    }

    /// Exact key authorized for this subject and Launch.
    pub(crate) fn baseline_key(owner: &SubjectId, launch: &LaunchId) -> String {
        let owner_hash = hex(&Sha256::digest(owner.0.as_bytes()));
        format!(
            "trigger-workspaces/subjects/{owner_hash}/launches/{}/baseline-v1.pack",
            launch.0
        )
    }

    fn import_prefix(owner: &SubjectId, launch: &LaunchId) -> String {
        let owner_hash = hex(&Sha256::digest(owner.0.as_bytes()));
        format!(
            "trigger-workspaces/subjects/{owner_hash}/launches/{}/baseline-import-v1",
            launch.0
        )
    }

    fn import_index_key(owner: &SubjectId, launch: &LaunchId) -> String {
        format!("{}/staging.json", Self::import_prefix(owner, launch))
    }

    fn import_completed_key(owner: &SubjectId, launch: &LaunchId) -> String {
        format!("{}/completed.json", Self::import_prefix(owner, launch))
    }

    fn import_retired_key(owner: &SubjectId, launch: &LaunchId) -> String {
        format!("{}/chunks-retired", Self::import_prefix(owner, launch))
    }

    fn import_chunk_key(
        owner: &SubjectId,
        launch: &LaunchId,
        source_revision: &str,
        provider_object_id: &str,
        ordinal: u32,
    ) -> Result<String, TriggerStoreError> {
        require_git_sha(source_revision, "baseline import source")?;
        require_git_sha(provider_object_id, "baseline import object")?;
        Ok(format!(
            "{}/sources/{source_revision}/blobs/{provider_object_id}/{ordinal:08}.part",
            Self::import_prefix(owner, launch)
        ))
    }

    /// Immutable object key for one Launch checkpoint version. The opaque
    /// checkpoint ID is hashed so it cannot become path syntax.
    pub(crate) fn checkpoint_key(owner: &SubjectId, checkpoint: &WorkspaceCheckpoint) -> String {
        Self::checkpoint_identity_key(
            owner,
            &checkpoint.launch_id,
            checkpoint.version,
            &checkpoint.id,
        )
    }

    fn checkpoint_intent_key(
        owner: &SubjectId,
        launch: &LaunchId,
        intent: &WorkspaceCheckpointIntent,
    ) -> String {
        Self::checkpoint_identity_key(owner, launch, intent.version, &intent.id)
    }

    fn recovery_conversation_intent_key(
        owner: &SubjectId,
        launch: &LaunchId,
        intent: &WorkspaceCheckpointIntent,
    ) -> String {
        Self::recovery_conversation_identity_key(owner, launch, intent.version, &intent.id)
    }

    fn checkpoint_identity_key(
        owner: &SubjectId,
        launch: &LaunchId,
        version: u64,
        checkpoint_id: &layer2_core::CheckpointId,
    ) -> String {
        let owner_hash = hex(&Sha256::digest(owner.0.as_bytes()));
        let checkpoint_hash = hex(&Sha256::digest(checkpoint_id.0.as_bytes()));
        format!(
            "trigger-workspaces/subjects/{owner_hash}/launches/{}/checkpoints/v{}-{checkpoint_hash}.pack",
            launch.0, version,
        )
    }

    fn recovery_conversation_identity_key(
        owner: &SubjectId,
        launch: &LaunchId,
        version: u64,
        checkpoint_id: &layer2_core::CheckpointId,
    ) -> String {
        let owner_hash = hex(&Sha256::digest(owner.0.as_bytes()));
        let checkpoint_hash = hex(&Sha256::digest(checkpoint_id.0.as_bytes()));
        format!(
            "trigger-workspaces/subjects/{owner_hash}/launches/{}/checkpoints/v{}-{checkpoint_hash}.conversation.snapshot",
            launch.0, version,
        )
    }

    fn recovery_conversation_key(owner: &SubjectId, checkpoint: &WorkspaceCheckpoint) -> String {
        Self::recovery_conversation_identity_key(
            owner,
            &checkpoint.launch_id,
            checkpoint.version,
            &checkpoint.id,
        )
    }

    pub(crate) fn diff_key(
        owner: &SubjectId,
        launch: &LaunchId,
        artifact: &DiffArtifactId,
        sha256: &str,
    ) -> String {
        let owner_hash = hex(&Sha256::digest(owner.0.as_bytes()));
        let artifact_hash = hex(&Sha256::digest(artifact.0.as_bytes()));
        format!(
            "trigger-workspaces/subjects/{owner_hash}/launches/{}/diffs/{artifact_hash}-{sha256}.json",
            launch.0
        )
    }

    fn require_reference(
        owner: &SubjectId,
        launch: &LaunchId,
        reference: &str,
    ) -> Result<String, TriggerStoreError> {
        let expected = Self::baseline_key(owner, launch);
        if reference != expected {
            return Err(TriggerStoreError::Malformed(
                "workspace object does not belong to this subject and Launch".into(),
            ));
        }
        Ok(expected)
    }

    fn require_checkpoint_reference(
        owner: &SubjectId,
        checkpoint: &WorkspaceCheckpoint,
    ) -> Result<String, TriggerStoreError> {
        let expected = Self::checkpoint_key(owner, checkpoint);
        if checkpoint.object_reference != expected {
            return Err(TriggerStoreError::Malformed(
                "checkpoint object does not belong to this subject and Launch".into(),
            ));
        }
        Ok(expected)
    }

    fn require_recovery_conversation_reference(
        owner: &SubjectId,
        checkpoint: &WorkspaceCheckpoint,
        pair: &TriggerRecoveryPairRef,
    ) -> Result<String, TriggerStoreError> {
        pair.validate_checkpoint(checkpoint)
            .map_err(|error| TriggerStoreError::Malformed(error.to_string()))?;
        let expected = Self::recovery_conversation_key(owner, checkpoint);
        if pair.conversation_object_reference != expected {
            return Err(TriggerStoreError::Malformed(
                "conversation checkpoint does not belong to this subject and Launch".into(),
            ));
        }
        Ok(expected)
    }

    async fn token(&self) -> Result<String, TriggerStoreError> {
        self.tokens
            .token()
            .await
            .map_err(|error| TriggerStoreError::Unavailable(format!("storage identity: {error}")))
    }

    async fn read_raw(&self, key: &str, max: u64) -> Result<Option<Vec<u8>>, TriggerStoreError> {
        let url = self.download_url();
        let response = self
            .http
            .get(&url)
            .query(&[("key", key)])
            .bearer_auth(self.token().await?)
            .send()
            .await
            .map_err(|error| {
                TriggerStoreError::Unavailable(format!("download workspace object: {error}"))
            })?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(TriggerStoreError::Unavailable(format!(
                "workspace download returned {}",
                response.status()
            )));
        }
        if response.content_length().is_some_and(|length| length > max) {
            return Err(TriggerStoreError::Malformed(
                "workspace object exceeds the configured archive bound".into(),
            ));
        }
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| {
                TriggerStoreError::Unavailable(format!("stream workspace object: {error}"))
            })?;
            let next = u64::try_from(bytes.len())
                .unwrap_or(u64::MAX)
                .saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
            if next > max {
                return Err(TriggerStoreError::Malformed(
                    "workspace object exceeds the configured archive bound".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(Some(bytes))
    }

    async fn read_raw_digest(
        &self,
        key: &str,
        max: u64,
    ) -> Result<Option<(u64, String)>, TriggerStoreError> {
        let response = self
            .http
            .get(self.download_url())
            .query(&[("key", key)])
            .bearer_auth(self.token().await?)
            .send()
            .await
            .map_err(|error| {
                TriggerStoreError::Unavailable(format!("download workspace object: {error}"))
            })?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(TriggerStoreError::Unavailable(format!(
                "workspace download returned {}",
                response.status()
            )));
        }
        if response.content_length().is_some_and(|length| length > max) {
            return Err(TriggerStoreError::Malformed(
                "workspace object exceeds the configured archive bound".into(),
            ));
        }
        let mut digest = Sha256::new();
        let mut total = 0_u64;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| {
                TriggerStoreError::Unavailable(format!("stream workspace object: {error}"))
            })?;
            total = total.checked_add(chunk.len() as u64).ok_or_else(|| {
                TriggerStoreError::Malformed("workspace object overflowed".into())
            })?;
            if total > max {
                return Err(TriggerStoreError::Malformed(
                    "workspace object exceeds the configured archive bound".into(),
                ));
            }
            digest.update(&chunk);
        }
        Ok(Some((total, hex(&digest.finalize()))))
    }

    async fn read_import_json<T: for<'de> Deserialize<'de>>(
        &self,
        key: &str,
    ) -> Result<Option<T>, TriggerStoreError> {
        self.read_raw(key, IMPORT_INDEX_MAX_BYTES)
            .await?
            .map(|bytes| {
                serde_json::from_slice(&bytes).map_err(|_| {
                    TriggerStoreError::Malformed("baseline import checkpoint is malformed".into())
                })
            })
            .transpose()
    }

    async fn write_import_json<T: Serialize>(
        &self,
        key: &str,
        value: &T,
    ) -> Result<(), TriggerStoreError> {
        let bytes = serde_json::to_vec(value).map_err(|_| {
            TriggerStoreError::Malformed("baseline import checkpoint cannot be encoded".into())
        })?;
        if bytes.len() as u64 > IMPORT_INDEX_MAX_BYTES {
            return Err(TriggerStoreError::Malformed(
                "baseline import checkpoint exceeds its bound".into(),
            ));
        }
        let sha256 = digest(&bytes);
        self.write_verified(
            key,
            IMPORT_INDEX_CONTENT_TYPE,
            &sha256,
            &bytes,
            IMPORT_INDEX_MAX_BYTES,
        )
        .await
    }

    fn canonical_import_blobs(
        blobs: &[BaselineImportBlob],
    ) -> Result<Vec<BaselineImportBlob>, TriggerStoreError> {
        if blobs.len() > MAX_REPOSITORY_FILES {
            return Err(TriggerStoreError::Malformed(
                "baseline import inventory exceeds the repository file bound".into(),
            ));
        }
        let mut unique = BTreeMap::new();
        let mut total_bytes = 0_u64;
        for blob in blobs {
            let canonical =
                BaselineImportBlob::new(&blob.provider_object_id, blob.bytes).map_err(|error| {
                    TriggerStoreError::Malformed(format!("baseline import inventory: {error}"))
                })?;
            if &canonical != blob {
                return Err(TriggerStoreError::Malformed(
                    "baseline import inventory is not canonical".into(),
                ));
            }
            total_bytes = total_bytes.checked_add(blob.bytes).ok_or_else(|| {
                TriggerStoreError::Malformed("baseline import inventory overflowed".into())
            })?;
            if total_bytes > MAX_REPOSITORY_BYTES {
                return Err(TriggerStoreError::Malformed(
                    "baseline import inventory exceeds the repository byte bound".into(),
                ));
            }
            if unique
                .insert(blob.provider_object_id.clone(), blob.clone())
                .is_some()
            {
                return Err(TriggerStoreError::Malformed(
                    "baseline import inventory contains a duplicate provider object".into(),
                ));
            }
        }
        Ok(unique.into_values().collect())
    }

    fn validate_import_index(
        index: &BaselineImportIndex,
        launch: &LaunchId,
    ) -> Result<(), TriggerStoreError> {
        require_git_sha(&index.source_revision, "baseline import source")?;
        require_sha256_value(&index.tree_digest, "baseline import tree digest")?;
        if index.version != IMPORT_INDEX_VERSION || &index.launch_id != launch {
            return Err(TriggerStoreError::Malformed(
                "baseline import inventory belongs to a different Launch or version".into(),
            ));
        }
        if Self::canonical_import_blobs(&index.blobs)? != index.blobs {
            return Err(TriggerStoreError::Malformed(
                "baseline import inventory is not deterministically ordered".into(),
            ));
        }
        Ok(())
    }

    async fn read_import_index(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
    ) -> Result<Option<BaselineImportIndex>, TriggerStoreError> {
        let index = self
            .read_import_json::<BaselineImportIndex>(&Self::import_index_key(owner, launch))
            .await?;
        if let Some(index) = index.as_ref() {
            Self::validate_import_index(index, launch)?;
        }
        Ok(index)
    }

    async fn require_import_index(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
        source_revision: &str,
    ) -> Result<BaselineImportIndex, TriggerStoreError> {
        require_git_sha(source_revision, "baseline import source")?;
        let index = self
            .read_import_index(owner, launch)
            .await?
            .ok_or_else(|| {
                TriggerStoreError::Malformed(
                    "baseline import chunks have no durable inventory".into(),
                )
            })?;
        if index.source_revision != source_revision {
            return Err(TriggerStoreError::Malformed(
                "baseline import source identity changed".into(),
            ));
        }
        Ok(index)
    }

    fn require_inventory_blob<'a>(
        index: &'a BaselineImportIndex,
        blob: &BaselineImportBlob,
    ) -> Result<&'a BaselineImportBlob, TriggerStoreError> {
        let retained = index
            .blobs
            .binary_search_by(|candidate| {
                candidate
                    .provider_object_id
                    .as_str()
                    .cmp(blob.provider_object_id.as_str())
            })
            .ok()
            .and_then(|position| index.blobs.get(position))
            .ok_or_else(|| {
                TriggerStoreError::Malformed(
                    "baseline import chunk is outside the retained inventory".into(),
                )
            })?;
        if retained != blob {
            return Err(TriggerStoreError::Malformed(
                "baseline import provider metadata changed".into(),
            ));
        }
        Ok(retained)
    }

    fn expected_import_chunk_bytes(
        blob: &BaselineImportBlob,
        ordinal: u32,
    ) -> Result<u64, TriggerStoreError> {
        if ordinal >= blob.chunk_count {
            return Err(TriggerStoreError::Malformed(
                "baseline import chunk ordinal is outside the provider object".into(),
            ));
        }
        Ok(blob
            .bytes
            .saturating_sub(u64::from(ordinal).saturating_mul(BASELINE_IMPORT_CHUNK_BYTES))
            .min(BASELINE_IMPORT_CHUNK_BYTES))
    }

    async fn read_staged_chunk(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
        source_revision: &str,
        chunk: &BaselineImportChunk,
    ) -> Result<Vec<u8>, TriggerStoreError> {
        require_sha256_value(&chunk.sha256, "baseline import chunk digest")?;
        let key = Self::import_chunk_key(
            owner,
            launch,
            source_revision,
            &chunk.provider_object_id,
            chunk.ordinal,
        )?;
        let bytes = self
            .read_raw(&key, BASELINE_IMPORT_CHUNK_BYTES)
            .await?
            .ok_or_else(|| {
                TriggerStoreError::Unavailable(
                    "a required baseline import chunk is not readable".into(),
                )
            })?;
        if bytes.len() as u64 != chunk.bytes || digest(&bytes) != chunk.sha256 {
            return Err(TriggerStoreError::Malformed(
                "a retained baseline import chunk failed its size or digest".into(),
            ));
        }
        Ok(bytes)
    }

    async fn archive_plan_digest(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
        plan: &BaselineArchivePlan,
    ) -> Result<String, TriggerStoreError> {
        let mut digest = Sha256::new();
        let mut bytes = plan.preamble.len() as u64;
        digest.update(&plan.preamble);
        for chunk in &plan.chunks {
            let retained = self
                .read_staged_chunk(owner, launch, &plan.source_revision, chunk)
                .await?;
            bytes = bytes.checked_add(retained.len() as u64).ok_or_else(|| {
                TriggerStoreError::Malformed("baseline archive byte count overflowed".into())
            })?;
            if bytes > workspace_archive_limit() {
                return Err(TriggerStoreError::Malformed(
                    "baseline archive exceeds the configured bound".into(),
                ));
            }
            digest.update(&retained);
        }
        if bytes != plan.archive_bytes {
            return Err(TriggerStoreError::Malformed(
                "baseline archive plan has the wrong byte count".into(),
            ));
        }
        Ok(hex(&digest.finalize()))
    }

    async fn upload_archive_plan(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
        plan: &BaselineArchivePlan,
        archive_sha256: &str,
    ) -> Result<(), TriggerStoreError> {
        let key = Self::baseline_key(owner, launch);
        let state = BaselineUploadStreamState {
            store: self.clone(),
            owner: owner.clone(),
            launch: launch.clone(),
            source_revision: plan.source_revision.clone(),
            preamble: Some(plan.preamble.clone()),
            chunks: plan.chunks.clone().into_iter(),
        };
        let stream = futures_util::stream::try_unfold(state, |mut state| async move {
            if let Some(preamble) = state.preamble.take() {
                return Ok::<_, TriggerStoreError>(Some((preamble, state)));
            }
            let Some(chunk) = state.chunks.next() else {
                return Ok(None);
            };
            let bytes = state
                .store
                .read_staged_chunk(&state.owner, &state.launch, &state.source_revision, &chunk)
                .await?;
            Ok(Some((bytes, state)))
        });
        let response = self
            .http
            .post(self.objects_url())
            .query(&[
                ("key", key.as_str()),
                ("contentType", WORKSPACE_CONTENT_TYPE),
            ])
            .bearer_auth(self.token().await?)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .header(reqwest::header::CONTENT_LENGTH, plan.archive_bytes)
            .body(reqwest::Body::wrap_stream(stream))
            .send()
            .await;
        let upload_succeeded =
            matches!(response.as_ref(), Ok(response) if response.status().is_success());
        let retained = self
            .read_raw_digest(&key, workspace_archive_limit())
            .await?;
        if retained == Some((plan.archive_bytes, archive_sha256.to_string())) {
            return Ok(());
        }
        match response {
            Ok(response) if !upload_succeeded => Err(TriggerStoreError::Unavailable(format!(
                "workspace upload returned {}",
                response.status()
            ))),
            Err(error) => Err(TriggerStoreError::Unavailable(format!(
                "upload workspace object: {error}"
            ))),
            Ok(_) => Err(TriggerStoreError::Malformed(
                "workspace destination did not retain the complete streamed archive".into(),
            )),
        }
    }

    async fn write_verified(
        &self,
        key: &str,
        content_type: &str,
        archive_sha256: &str,
        archive: &[u8],
        max: u64,
    ) -> Result<(), TriggerStoreError> {
        require_sha256(archive_sha256)?;
        if digest(archive) != archive_sha256 {
            return Err(TriggerStoreError::Malformed(
                "workspace archive does not match its SHA-256".into(),
            ));
        }
        if u64::try_from(archive.len()).unwrap_or(u64::MAX) > max {
            return Err(TriggerStoreError::Malformed(
                "workspace archive exceeds the configured bound".into(),
            ));
        }
        if let Some(retained) = self.read_raw(key, max).await? {
            if digest(&retained) == archive_sha256 {
                return Ok(());
            }
            return Err(TriggerStoreError::Malformed(
                "workspace object key already names different retained bytes".into(),
            ));
        }
        let response = self
            .http
            .post(self.objects_url())
            .query(&[("key", key), ("contentType", content_type)])
            .bearer_auth(self.token().await?)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(archive.to_vec())
            .send()
            .await
            .map_err(|error| {
                TriggerStoreError::Unavailable(format!("upload workspace object: {error}"))
            })?;
        if !response.status().is_success() {
            return Err(TriggerStoreError::Unavailable(format!(
                "workspace upload returned {}",
                response.status()
            )));
        }
        let retained = self.read_raw(key, max).await?.ok_or_else(|| {
            TriggerStoreError::Unavailable(
                "workspace upload was acknowledged but no object was readable".into(),
            )
        })?;
        if digest(&retained) != archive_sha256 {
            return Err(TriggerStoreError::Malformed(
                "workspace destination did not retain the complete archive".into(),
            ));
        }
        Ok(())
    }

    async fn delete_raw(&self, key: &str) -> Result<(), TriggerStoreError> {
        let response = self
            .http
            .delete(self.objects_url())
            .query(&[("key", key)])
            .bearer_auth(self.token().await?)
            .send()
            .await
            .map_err(|error| {
                TriggerStoreError::Unavailable(format!("delete workspace object: {error}"))
            })?;
        if response.status().is_success() || response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        Err(TriggerStoreError::Unavailable(format!(
            "workspace delete returned {}",
            response.status()
        )))
    }

    async fn delete_raw_verified(&self, key: &str, max: u64) -> Result<(), TriggerStoreError> {
        let deletion = self.delete_raw(key).await;
        match self.read_raw(key, max).await {
            Ok(None) => Ok(()),
            Ok(Some(_)) => match deletion {
                Err(error) => Err(error),
                Ok(()) => Err(TriggerStoreError::Unavailable(
                    "workspace deletion was acknowledged but the object remains readable".into(),
                )),
            },
            Err(read_error) => match deletion {
                Err(delete_error) => Err(delete_error),
                Ok(()) => Err(read_error),
            },
        }
    }
}

#[async_trait::async_trait]
impl BaselineObjectStore for ChronoStorageTriggerWorkspaceStore {
    async fn reconcile_baseline_import(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
        source_revision: &str,
    ) -> Result<Option<BaselineImportOutcome>, TriggerStoreError> {
        require_git_sha(source_revision, "baseline import source")?;
        let Some(completed) = self
            .read_import_json::<CompletedBaselineImport>(&Self::import_completed_key(owner, launch))
            .await?
        else {
            return Ok(None);
        };
        if completed.version != IMPORT_INDEX_VERSION
            || &completed.launch_id != launch
            || completed.source_revision != source_revision
        {
            return Err(TriggerStoreError::Malformed(
                "completed baseline import identity changed".into(),
            ));
        }
        require_sha256_value(&completed.tree_digest, "baseline import tree digest")?;
        let index = self
            .require_import_index(owner, launch, source_revision)
            .await?;
        if completed.tree_digest != index.tree_digest {
            return Err(TriggerStoreError::Malformed(
                "completed baseline import disagrees with its provider inventory".into(),
            ));
        }
        completed.outcome.manifest.validate().map_err(|error| {
            TriggerStoreError::Malformed(format!("completed baseline manifest: {error}"))
        })?;
        if completed.outcome.manifest.source_revision != source_revision
            || completed.outcome.object_reference != Self::baseline_key(owner, launch)
            || completed.outcome.archive_bytes > workspace_archive_limit()
        {
            return Err(TriggerStoreError::Malformed(
                "completed baseline import metadata changed".into(),
            ));
        }
        require_sha256_value(
            &completed.outcome.archive_sha256,
            "completed baseline archive digest",
        )?;
        let retained = self
            .read_raw_digest(
                &completed.outcome.object_reference,
                workspace_archive_limit(),
            )
            .await?
            .ok_or_else(|| {
                TriggerStoreError::Malformed(
                    "completed baseline import no longer has a retained archive".into(),
                )
            })?;
        if retained
            != (
                completed.outcome.archive_bytes,
                completed.outcome.archive_sha256.clone(),
            )
        {
            return Err(TriggerStoreError::Malformed(
                "completed baseline archive failed its retained size or digest".into(),
            ));
        }
        Ok(Some(completed.outcome))
    }

    async fn begin_baseline_import(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
        source_revision: &str,
        tree_digest: &str,
        blobs: &[BaselineImportBlob],
    ) -> Result<(), TriggerStoreError> {
        require_git_sha(source_revision, "baseline import source")?;
        require_sha256_value(tree_digest, "baseline import tree digest")?;
        let blobs = Self::canonical_import_blobs(blobs)?;
        let index = BaselineImportIndex {
            version: IMPORT_INDEX_VERSION,
            launch_id: launch.clone(),
            source_revision: source_revision.to_string(),
            tree_digest: tree_digest.to_string(),
            blobs,
        };
        if let Some(retained) = self.read_import_index(owner, launch).await? {
            if retained == index {
                return Ok(());
            }
            return Err(TriggerStoreError::Malformed(
                "baseline import inventory changed after staging began".into(),
            ));
        }
        if self
            .read_raw(
                &Self::import_retired_key(owner, launch),
                IMPORT_RETIRED_MARKER.len() as u64,
            )
            .await?
            .is_some()
        {
            return Err(TriggerStoreError::Malformed(
                "baseline import was retired without its durable inventory".into(),
            ));
        }
        self.write_import_json(&Self::import_index_key(owner, launch), &index)
            .await
    }

    async fn get_baseline_import_chunk(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
        source_revision: &str,
        blob: &BaselineImportBlob,
        ordinal: u32,
    ) -> Result<Option<Vec<u8>>, TriggerStoreError> {
        let index = self
            .require_import_index(owner, launch, source_revision)
            .await?;
        let blob = Self::require_inventory_blob(&index, blob)?;
        let expected = Self::expected_import_chunk_bytes(blob, ordinal)?;
        let key = Self::import_chunk_key(
            owner,
            launch,
            source_revision,
            &blob.provider_object_id,
            ordinal,
        )?;
        let Some(bytes) = self.read_raw(&key, BASELINE_IMPORT_CHUNK_BYTES).await? else {
            return Ok(None);
        };
        if bytes.len() as u64 != expected {
            return Err(TriggerStoreError::Malformed(
                "a retained baseline import chunk has the wrong size".into(),
            ));
        }
        Ok(Some(bytes))
    }

    async fn put_baseline_import_chunk(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
        source_revision: &str,
        blob: &BaselineImportBlob,
        chunk: &BaselineImportChunk,
        bytes: &[u8],
    ) -> Result<(), TriggerStoreError> {
        let index = self
            .require_import_index(owner, launch, source_revision)
            .await?;
        let blob = Self::require_inventory_blob(&index, blob)?;
        let expected = Self::expected_import_chunk_bytes(blob, chunk.ordinal)?;
        require_sha256_value(&chunk.sha256, "baseline import chunk digest")?;
        if chunk.provider_object_id != blob.provider_object_id
            || chunk.bytes != expected
            || bytes.len() as u64 != expected
            || digest(bytes) != chunk.sha256
        {
            return Err(TriggerStoreError::Malformed(
                "baseline import chunk does not match its immutable identity".into(),
            ));
        }
        let key = Self::import_chunk_key(
            owner,
            launch,
            source_revision,
            &blob.provider_object_id,
            chunk.ordinal,
        )?;
        self.write_verified(
            &key,
            IMPORT_CHUNK_CONTENT_TYPE,
            &chunk.sha256,
            bytes,
            BASELINE_IMPORT_CHUNK_BYTES,
        )
        .await
    }

    async fn finalize_baseline_import(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
        manifest: &WorkspaceManifest,
        plan: BaselineArchivePlan,
    ) -> Result<BaselineImportOutcome, TriggerStoreError> {
        manifest.validate().map_err(|error| {
            TriggerStoreError::Malformed(format!("workspace manifest: {error}"))
        })?;
        require_git_sha(&plan.source_revision, "baseline import source")?;
        require_sha256_value(&plan.tree_digest, "baseline import tree digest")?;
        if manifest.source_revision != plan.source_revision {
            return Err(TriggerStoreError::Malformed(
                "baseline archive source disagrees with its manifest".into(),
            ));
        }
        let index = self
            .require_import_index(owner, launch, &plan.source_revision)
            .await?;
        if index.tree_digest != plan.tree_digest {
            return Err(TriggerStoreError::Malformed(
                "baseline archive plan disagrees with its retained provider inventory".into(),
            ));
        }
        if plan.preamble.len() < 8
            || plan.preamble.len() as u64 > ARCHIVE_OVERHEAD_BYTES
            || plan.archive_bytes > workspace_archive_limit()
        {
            return Err(TriggerStoreError::Malformed(
                "baseline archive preamble is outside its configured bound".into(),
            ));
        }
        let header_bytes = u64::from_be_bytes(
            plan.preamble[..8]
                .try_into()
                .expect("the preamble length was checked"),
        );
        if header_bytes != (plan.preamble.len() - 8) as u64 {
            return Err(TriggerStoreError::Malformed(
                "baseline archive preamble has an invalid header length".into(),
            ));
        }
        let header: serde_json::Value =
            serde_json::from_slice(&plan.preamble[8..]).map_err(|_| {
                TriggerStoreError::Malformed("baseline archive header is malformed".into())
            })?;
        if header
            .get("cma_workspace")
            .and_then(serde_json::Value::as_u64)
            != Some(1)
            || header
                .get("workspace_epoch")
                .and_then(serde_json::Value::as_u64)
                != Some(1)
            || header
                .get("source_revision")
                .and_then(serde_json::Value::as_str)
                != Some(manifest.source_revision.as_str())
            || header
                .get("manifest_digest")
                .and_then(serde_json::Value::as_str)
                != Some(manifest.digest.as_str())
            || header
                .get("entries")
                .and_then(serde_json::Value::as_array)
                .is_none_or(|entries| entries.len() != manifest.entries.len())
        {
            return Err(TriggerStoreError::Malformed(
                "baseline archive header disagrees with its sealed manifest".into(),
            ));
        }

        let inventory = index
            .blobs
            .iter()
            .map(|blob| (blob.provider_object_id.as_str(), blob))
            .collect::<BTreeMap<_, _>>();
        let mut used = std::collections::BTreeSet::new();
        let mut cursor = 0_usize;
        let mut payload_bytes = 0_u64;
        for entry in &manifest.entries {
            let provider_object_id = entry.provider_object_id.as_deref().ok_or_else(|| {
                TriggerStoreError::Malformed(
                    "baseline manifest entry has no provider object identity".into(),
                )
            })?;
            if entry.kind == WorkspaceEntryKind::Submodule {
                // A gitlink names a commit, not a blob. Its zero-byte archive
                // entry preserves the pinned identity without placing that
                // commit in the fetchable provider-blob inventory.
                continue;
            }
            let blob = inventory.get(provider_object_id).copied().ok_or_else(|| {
                TriggerStoreError::Malformed(
                    "baseline manifest entry is outside the retained provider inventory".into(),
                )
            })?;
            if entry.size != blob.bytes {
                return Err(TriggerStoreError::Malformed(
                    "baseline manifest entry size changed after provider discovery".into(),
                ));
            }
            used.insert(provider_object_id);
            for ordinal in 0..blob.chunk_count {
                let chunk = plan.chunks.get(cursor).ok_or_else(|| {
                    TriggerStoreError::Malformed(
                        "baseline archive plan omits a provider chunk".into(),
                    )
                })?;
                let expected = Self::expected_import_chunk_bytes(blob, ordinal)?;
                require_sha256_value(&chunk.sha256, "baseline import chunk digest")?;
                if chunk.provider_object_id != provider_object_id
                    || chunk.ordinal != ordinal
                    || chunk.bytes != expected
                {
                    return Err(TriggerStoreError::Malformed(
                        "baseline archive chunk order changed".into(),
                    ));
                }
                payload_bytes = payload_bytes.checked_add(chunk.bytes).ok_or_else(|| {
                    TriggerStoreError::Malformed("baseline archive byte count overflowed".into())
                })?;
                cursor += 1;
            }
        }
        if cursor != plan.chunks.len()
            || used.len() != inventory.len()
            || payload_bytes != manifest.total_bytes
            || plan.archive_bytes
                != (plan.preamble.len() as u64).saturating_add(manifest.total_bytes)
        {
            return Err(TriggerStoreError::Malformed(
                "baseline archive plan does not exactly cover its manifest".into(),
            ));
        }

        let archive_sha256 = self.archive_plan_digest(owner, launch, &plan).await?;
        let object_reference = Self::baseline_key(owner, launch);
        match self
            .read_raw_digest(&object_reference, workspace_archive_limit())
            .await?
        {
            Some(retained) if retained == (plan.archive_bytes, archive_sha256.clone()) => {}
            Some(_) => {
                return Err(TriggerStoreError::Malformed(
                    "baseline object key already names different retained bytes".into(),
                ));
            }
            None => {
                self.upload_archive_plan(owner, launch, &plan, &archive_sha256)
                    .await?;
            }
        }
        let outcome = BaselineImportOutcome {
            manifest: manifest.clone(),
            object_reference,
            archive_sha256,
            archive_bytes: plan.archive_bytes,
        };
        let completed = CompletedBaselineImport {
            version: IMPORT_INDEX_VERSION,
            launch_id: launch.clone(),
            source_revision: plan.source_revision,
            tree_digest: plan.tree_digest,
            outcome: outcome.clone(),
        };
        self.write_import_json(&Self::import_completed_key(owner, launch), &completed)
            .await?;
        Ok(outcome)
    }

    async fn retire_baseline_import_chunks(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
    ) -> Result<(), TriggerStoreError> {
        let marker_key = Self::import_retired_key(owner, launch);
        if let Some(marker) = self
            .read_raw(&marker_key, IMPORT_RETIRED_MARKER.len() as u64)
            .await?
        {
            if marker == IMPORT_RETIRED_MARKER {
                return Ok(());
            }
            return Err(TriggerStoreError::Malformed(
                "baseline import retirement marker is malformed".into(),
            ));
        }
        let index = self
            .read_import_index(owner, launch)
            .await?
            .ok_or_else(|| {
                TriggerStoreError::Malformed("baseline import has no retirement inventory".into())
            })?;
        if self
            .reconcile_baseline_import(owner, launch, &index.source_revision)
            .await?
            .is_none()
        {
            return Err(TriggerStoreError::Malformed(
                "baseline import chunks cannot retire before finalization".into(),
            ));
        }
        for blob in &index.blobs {
            for ordinal in 0..blob.chunk_count {
                let key = Self::import_chunk_key(
                    owner,
                    launch,
                    &index.source_revision,
                    &blob.provider_object_id,
                    ordinal,
                )?;
                self.delete_raw_verified(&key, BASELINE_IMPORT_CHUNK_BYTES)
                    .await?;
            }
        }
        let marker_sha256 = digest(IMPORT_RETIRED_MARKER);
        self.write_verified(
            &marker_key,
            IMPORT_INDEX_CONTENT_TYPE,
            &marker_sha256,
            IMPORT_RETIRED_MARKER,
            IMPORT_RETIRED_MARKER.len() as u64,
        )
        .await
    }

    async fn delete_baseline_import(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
    ) -> Result<(), TriggerStoreError> {
        if let Some(index) = self.read_import_index(owner, launch).await? {
            for blob in &index.blobs {
                for ordinal in 0..blob.chunk_count {
                    let key = Self::import_chunk_key(
                        owner,
                        launch,
                        &index.source_revision,
                        &blob.provider_object_id,
                        ordinal,
                    )?;
                    self.delete_raw_verified(&key, BASELINE_IMPORT_CHUNK_BYTES)
                        .await?;
                }
            }
        }
        self.delete_raw_verified(
            &Self::import_retired_key(owner, launch),
            IMPORT_RETIRED_MARKER.len() as u64,
        )
        .await?;
        self.delete_raw_verified(
            &Self::import_completed_key(owner, launch),
            IMPORT_INDEX_MAX_BYTES,
        )
        .await?;
        self.delete_raw_verified(
            &Self::import_index_key(owner, launch),
            IMPORT_INDEX_MAX_BYTES,
        )
        .await
    }

    async fn put_baseline(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
        manifest: &WorkspaceManifest,
        archive_sha256: &str,
        archive: &[u8],
    ) -> Result<String, TriggerStoreError> {
        manifest.validate().map_err(|error| {
            TriggerStoreError::Malformed(format!("workspace manifest: {error}"))
        })?;
        let key = Self::baseline_key(owner, launch);
        self.write_verified(
            &key,
            WORKSPACE_CONTENT_TYPE,
            archive_sha256,
            archive,
            workspace_archive_limit(),
        )
        .await?;
        Ok(key)
    }

    async fn get_baseline(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
        reference: &str,
        archive_sha256: &str,
    ) -> Result<Option<Vec<u8>>, TriggerStoreError> {
        let key = Self::require_reference(owner, launch, reference)?;
        require_sha256(archive_sha256)?;
        let Some(bytes) = self.read_raw(&key, workspace_archive_limit()).await? else {
            return Ok(None);
        };
        if digest(&bytes) != archive_sha256 {
            return Err(TriggerStoreError::Malformed(
                "workspace object failed its retained SHA-256".into(),
            ));
        }
        Ok(Some(bytes))
    }

    async fn delete_baseline(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
        reference: &str,
    ) -> Result<(), TriggerStoreError> {
        let key = Self::require_reference(owner, launch, reference)?;
        self.delete_raw_verified(&key, workspace_archive_limit())
            .await?;
        self.delete_baseline_import(owner, launch).await
    }
}

impl ChronoStorageTriggerWorkspaceStore {
    async fn put_recovery_conversation(
        &self,
        owner: &SubjectId,
        checkpoint: &WorkspaceCheckpoint,
        conversation: &[u8],
    ) -> Result<String, TriggerStoreError> {
        let pair = checkpoint.recovery_pair.as_ref().ok_or_else(|| {
            TriggerStoreError::Malformed(
                "a recoverable checkpoint has no conversation descriptor".into(),
            )
        })?;
        let key = Self::require_recovery_conversation_reference(owner, checkpoint, pair)?;
        if u64::try_from(conversation.len()).unwrap_or(u64::MAX) != pair.conversation_bytes
            || conversation.is_empty()
            || pair.conversation_bytes > MAX_TRIGGER_CONVERSATION_SNAPSHOT_BYTES
            || digest(conversation) != pair.conversation_sha256
        {
            return Err(TriggerStoreError::Malformed(
                "conversation checkpoint failed its retained size or SHA-256".into(),
            ));
        }
        self.write_verified(
            &key,
            RECOVERY_CONVERSATION_CONTENT_TYPE,
            &pair.conversation_sha256,
            conversation,
            MAX_TRIGGER_CONVERSATION_SNAPSHOT_BYTES,
        )
        .await?;
        Ok(key)
    }

    async fn get_recovery_conversation(
        &self,
        owner: &SubjectId,
        checkpoint: &WorkspaceCheckpoint,
    ) -> Result<Option<Vec<u8>>, TriggerStoreError> {
        let pair = checkpoint.recovery_pair.as_ref().ok_or_else(|| {
            TriggerStoreError::Malformed(
                "the checkpoint is workspace-only and cannot recover a conversation".into(),
            )
        })?;
        let key = Self::require_recovery_conversation_reference(owner, checkpoint, pair)?;
        let Some(bytes) = self
            .read_raw(&key, MAX_TRIGGER_CONVERSATION_SNAPSHOT_BYTES)
            .await?
        else {
            return Ok(None);
        };
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != pair.conversation_bytes
            || digest(&bytes) != pair.conversation_sha256
        {
            return Err(TriggerStoreError::Malformed(
                "conversation checkpoint failed its retained size or SHA-256".into(),
            ));
        }
        Ok(Some(bytes))
    }

    async fn delete_recovery_conversation(
        &self,
        owner: &SubjectId,
        checkpoint: &WorkspaceCheckpoint,
    ) -> Result<(), TriggerStoreError> {
        let Some(pair) = checkpoint.recovery_pair.as_ref() else {
            return Ok(());
        };
        let key = Self::require_recovery_conversation_reference(owner, checkpoint, pair)?;
        self.delete_raw_verified(&key, MAX_TRIGGER_CONVERSATION_SNAPSHOT_BYTES)
            .await
    }
}

#[async_trait::async_trait]
impl CheckpointObjectStore for ChronoStorageTriggerWorkspaceStore {
    async fn put_checkpoint(
        &self,
        owner: &SubjectId,
        checkpoint: &WorkspaceCheckpoint,
        archive: &[u8],
    ) -> Result<String, TriggerStoreError> {
        let key = Self::checkpoint_key(owner, checkpoint);
        if !checkpoint.object_reference.is_empty() {
            Self::require_checkpoint_reference(owner, checkpoint)?;
        }
        let mut candidate = checkpoint.clone();
        candidate.object_reference = key.clone();
        candidate
            .validate_complete(&candidate.launch_id)
            .map_err(|error| TriggerStoreError::Malformed(error.to_string()))?;
        self.write_verified(
            &key,
            WORKSPACE_CONTENT_TYPE,
            &checkpoint.archive_sha256,
            archive,
            workspace_archive_limit(),
        )
        .await?;
        Ok(key)
    }

    async fn get_checkpoint(
        &self,
        owner: &SubjectId,
        checkpoint: &WorkspaceCheckpoint,
    ) -> Result<Option<Vec<u8>>, TriggerStoreError> {
        let key = Self::require_checkpoint_reference(owner, checkpoint)?;
        let Some(bytes) = self.read_raw(&key, workspace_archive_limit()).await? else {
            return Ok(None);
        };
        if digest(&bytes) != checkpoint.archive_sha256 {
            return Err(TriggerStoreError::Malformed(
                "checkpoint object failed its retained SHA-256".into(),
            ));
        }
        Ok(Some(bytes))
    }

    async fn delete_checkpoint(
        &self,
        owner: &SubjectId,
        checkpoint: &WorkspaceCheckpoint,
    ) -> Result<(), TriggerStoreError> {
        let key = Self::require_checkpoint_reference(owner, checkpoint)?;
        self.delete_raw_verified(&key, workspace_archive_limit())
            .await?;
        self.delete_recovery_conversation(owner, checkpoint).await
    }

    async fn delete_checkpoint_intent(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
        intent: &WorkspaceCheckpointIntent,
    ) -> Result<(), TriggerStoreError> {
        if intent.id.0.is_empty() || intent.version == 0 {
            return Err(TriggerStoreError::Malformed(
                "checkpoint intent cannot address an object".into(),
            ));
        }
        self.delete_raw_verified(
            &Self::checkpoint_intent_key(owner, launch, intent),
            workspace_archive_limit(),
        )
        .await?;
        self.delete_raw_verified(
            &Self::recovery_conversation_intent_key(owner, launch, intent),
            MAX_TRIGGER_CONVERSATION_SNAPSHOT_BYTES,
        )
        .await
    }
}

#[async_trait::async_trait]
impl DiffArtifactObjectStore for ChronoStorageTriggerWorkspaceStore {
    async fn put_diff_artifact(
        &self,
        owner: &SubjectId,
        launch: &LaunchId,
        artifact: &DiffArtifactId,
        sha256: &str,
        bytes: &[u8],
    ) -> Result<String, TriggerStoreError> {
        require_sha256(sha256)?;
        if bytes.is_empty()
            || u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_DIFF_ARTIFACT_BYTES
        {
            return Err(TriggerStoreError::Malformed(
                "diff review artifact is outside its storage bound".into(),
            ));
        }
        let key = Self::diff_key(owner, launch, artifact, sha256);
        self.write_verified(
            &key,
            DIFF_CONTENT_TYPE,
            sha256,
            bytes,
            MAX_DIFF_ARTIFACT_BYTES,
        )
        .await?;
        Ok(key)
    }

    async fn get_diff_artifact(
        &self,
        owner: &SubjectId,
        artifact: &DiffArtifact,
    ) -> Result<Option<Vec<u8>>, TriggerStoreError> {
        let expected = Self::diff_key(
            owner,
            &artifact.launch_id,
            &artifact.id,
            &artifact.object_sha256,
        );
        if artifact.object_reference != expected || artifact.object_sha256 != artifact.digest {
            return Err(TriggerStoreError::Malformed(
                "diff artifact does not belong to this subject and Launch".into(),
            ));
        }
        let Some(bytes) = self.read_raw(&expected, MAX_DIFF_ARTIFACT_BYTES).await? else {
            return Ok(None);
        };
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != artifact.object_bytes
            || digest(&bytes) != artifact.object_sha256
        {
            return Err(TriggerStoreError::Malformed(
                "diff artifact failed its retained size or SHA-256".into(),
            ));
        }
        Ok(Some(bytes))
    }

    async fn delete_diff_artifact(
        &self,
        owner: &SubjectId,
        artifact: &DiffArtifact,
    ) -> Result<(), TriggerStoreError> {
        let expected = Self::diff_key(
            owner,
            &artifact.launch_id,
            &artifact.id,
            &artifact.object_sha256,
        );
        if artifact.object_reference != expected {
            return Err(TriggerStoreError::Malformed(
                "diff artifact does not belong to this subject and Launch".into(),
            ));
        }
        self.delete_raw_verified(&expected, MAX_DIFF_ARTIFACT_BYTES)
            .await
    }
}

#[async_trait::async_trait]
impl SessionWorkspaceSource for ChronoStorageTriggerWorkspaceStore {
    async fn load_workspace(
        &self,
        owner: &SubjectId,
        origin: &TriggerSessionOrigin,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        self.get_baseline(
            owner,
            &origin.launch_id,
            &origin.object_reference,
            &origin.archive_sha256,
        )
        .await
        .map_err(|error| match error {
            TriggerStoreError::Unavailable(detail) => StoreError::Unavailable(detail),
            other => StoreError::Malformed(other.to_string()),
        })
    }

    async fn resolve_workspace(
        &self,
        owner: &SubjectId,
        origin: &TriggerSessionOrigin,
    ) -> Result<Option<layer3_adapter::WorkspacePackage>, StoreError> {
        origin
            .validate()
            .map_err(|error| StoreError::Malformed(error.to_string()))?;
        let launch = self
            .launches
            .get_launch(&origin.launch_id)
            .await
            .map_err(workspace_store_error)?
            .ok_or_else(|| StoreError::Malformed("the Trigger Launch no longer exists".into()))?;
        let checkpoint = select_recovery_checkpoint(&launch, owner, origin)?;
        let workspace = launch.workspace.as_ref().expect("selection validates it");
        if let Some(checkpoint) = checkpoint {
            let archive = CheckpointObjectStore::get_checkpoint(self, owner, checkpoint)
                .await
                .map_err(workspace_store_error)?;
            return Ok(archive.map(|archive| layer3_adapter::WorkspacePackage {
                epoch: checkpoint.workspace_epoch,
                source_revision: checkpoint.manifest.source_revision.clone(),
                baseline_manifest_digest: workspace.baseline.digest.clone(),
                manifest_digest: checkpoint.manifest.digest.clone(),
                archive_sha256: checkpoint.archive_sha256.clone(),
                archive,
            }));
        }

        let archive = BaselineObjectStore::get_baseline(
            self,
            owner,
            &origin.launch_id,
            &origin.object_reference,
            &origin.archive_sha256,
        )
        .await
        .map_err(workspace_store_error)?;
        Ok(archive.map(|archive| layer3_adapter::WorkspacePackage {
            epoch: origin.workspace_epoch,
            source_revision: origin.source_revision.clone(),
            baseline_manifest_digest: origin.manifest_digest.clone(),
            manifest_digest: origin.manifest_digest.clone(),
            archive_sha256: origin.archive_sha256.clone(),
            archive,
        }))
    }

    async fn resolve_recovery_pair(
        &self,
        owner: &SubjectId,
        origin: &TriggerSessionOrigin,
        expected: Option<&TriggerRecoveryPairRef>,
    ) -> Result<Option<TriggerRecoveryPair>, StoreError> {
        origin
            .validate()
            .map_err(|error| StoreError::Malformed(error.to_string()))?;
        let launch = self
            .launches
            .get_launch(&origin.launch_id)
            .await
            .map_err(workspace_store_error)?
            .ok_or_else(|| StoreError::Malformed("the Trigger Launch no longer exists".into()))?;
        let Some(checkpoint) = select_recovery_checkpoint(&launch, owner, origin)? else {
            return Ok(None);
        };
        let workspace_record = launch
            .workspace
            .as_ref()
            .expect("checkpoint selection validates the Launch workspace");
        if workspace_record.lifecycle_checkpoint.state == LifecycleCheckpointState::RecoveryRequired
        {
            return Err(StoreError::Malformed(
                "the latest recovery pair requires explicit recovery because its runtime writer state is unproven"
                    .into(),
            ));
        }
        let Some(reference) = checkpoint.recovery_pair.as_ref() else {
            // Legacy workspace-only checkpoints remain valid for contribution
            // diffs but are never presented as conversation recovery points.
            return Ok(None);
        };
        reference
            .validate_checkpoint(checkpoint)
            .map_err(|error| StoreError::Malformed(error.to_string()))?;
        if expected.is_some_and(|expected| expected != reference) {
            return Err(StoreError::Malformed(
                "the requested recovery pair is not the latest complete checkpoint".into(),
            ));
        }
        let workspace_archive = CheckpointObjectStore::get_checkpoint(self, owner, checkpoint)
            .await
            .map_err(workspace_store_error)?
            .ok_or_else(|| {
                StoreError::Malformed("the recovery pair workspace object is missing".into())
            })?;
        let conversation = self
            .get_recovery_conversation(owner, checkpoint)
            .await
            .map_err(workspace_store_error)?
            .ok_or_else(|| {
                StoreError::Malformed("the recovery pair conversation object is missing".into())
            })?;
        let pair = TriggerRecoveryPair {
            reference: reference.clone(),
            workspace: layer3_adapter::WorkspacePackage {
                epoch: checkpoint.workspace_epoch,
                source_revision: checkpoint.manifest.source_revision.clone(),
                baseline_manifest_digest: workspace_record.baseline.digest.clone(),
                manifest_digest: checkpoint.manifest.digest.clone(),
                archive_sha256: checkpoint.archive_sha256.clone(),
                archive: workspace_archive,
            },
            conversation,
        };
        pair.validate_for(origin)
            .map_err(|error| StoreError::Malformed(error.to_string()))?;
        Ok(Some(pair))
    }

    async fn synchronize_runtime_generation(
        &self,
        owner: &SubjectId,
        origin: &TriggerSessionOrigin,
        runtime_generation: u64,
        now: &str,
    ) -> Result<(), StoreError> {
        origin
            .validate()
            .map_err(|error| StoreError::Malformed(error.to_string()))?;
        let mut launch = self
            .launches
            .get_launch(&origin.launch_id)
            .await
            .map_err(workspace_store_error)?
            .ok_or_else(|| StoreError::Malformed("the Trigger Launch no longer exists".into()))?;
        validate_launch_workspace_authority(&launch, owner, origin)?;
        let session_id = origin
            .session_id()
            .map_err(|error| StoreError::Malformed(error.to_string()))?;
        let changed = launch
            .synchronize_runtime_generation(&session_id, owner, runtime_generation, now)
            .map_err(|error| StoreError::Malformed(error.to_string()))?;
        if changed {
            replace_unleased_launch(self.launches.as_ref(), launch).await?;
        }
        Ok(())
    }

    async fn reserve_checkpoint(
        &self,
        owner: &SubjectId,
        origin: &TriggerSessionOrigin,
        runtime_generation: u64,
        purpose: WorkspaceCheckpointPurpose,
        now: &str,
    ) -> Result<WorkspaceCheckpointReservation, StoreError> {
        origin
            .validate()
            .map_err(|error| StoreError::Malformed(error.to_string()))?;
        let mut launch = self
            .launches
            .get_launch(&origin.launch_id)
            .await
            .map_err(workspace_store_error)?
            .ok_or_else(|| StoreError::Malformed("the Trigger Launch no longer exists".into()))?;
        validate_launch_workspace_authority(&launch, owner, origin)?;
        if launch
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.retired_checkpoint.as_ref())
            .is_some()
        {
            launch = self.settle_retired_checkpoint(owner, launch).await?;
        }
        let before = launch.clone();
        let session_id = origin
            .session_id()
            .map_err(|error| StoreError::Malformed(error.to_string()))?;
        let reservation = launch
            .reserve_lifecycle_checkpoint(purpose, &session_id, owner, runtime_generation, now)
            .map_err(|error| StoreError::Malformed(error.to_string()))?;
        if launch != before {
            replace_unleased_launch(self.launches.as_ref(), launch).await?;
        }
        Ok(reservation)
    }

    #[allow(clippy::too_many_arguments)] // The storage commit verifies one exact capture tuple.
    async fn commit_checkpoint(
        &self,
        owner: &SubjectId,
        origin: &TriggerSessionOrigin,
        intent: &WorkspaceCheckpointIntent,
        runtime_generation: u64,
        captured: &layer3_adapter::WorkspaceCheckpointPackage,
        conversation: &[u8],
        now: &str,
    ) -> Result<TriggerRecoveryPairCommitReceipt, StoreError> {
        if !intent.purpose.is_lifecycle()
            || runtime_generation != intent.runtime_generation
            || captured.epoch != origin.workspace_epoch
            || captured.version != intent.version
            || !captured
                .source_revision
                .eq_ignore_ascii_case(&origin.source_revision)
            || digest(&captured.archive) != captured.archive_sha256
            || conversation.is_empty()
            || u64::try_from(conversation.len()).unwrap_or(u64::MAX)
                > MAX_TRIGGER_CONVERSATION_SNAPSHOT_BYTES
        {
            return Err(StoreError::Malformed(
                "the captured checkpoint does not match its durable lifecycle intent".into(),
            ));
        }
        let entries = captured
            .entries
            .iter()
            .map(|entry| WorkspaceEntry {
                path: entry.path.clone(),
                kind: match entry.kind {
                    layer3_adapter::WorkspaceEntryKind::Regular => WorkspaceEntryKind::Regular,
                    layer3_adapter::WorkspaceEntryKind::Symlink => WorkspaceEntryKind::Symlink,
                },
                mode: entry.mode,
                size: entry.size,
                content_sha256: entry.content_sha256.clone(),
                provider_object_id: entry.provider_object_id.clone(),
                symlink_target: entry.symlink_target.clone(),
            })
            .collect::<Vec<_>>();
        let manifest = WorkspaceManifest::seal(captured.source_revision.clone(), entries)
            .map_err(|error| StoreError::Malformed(error.to_string()))?;
        if manifest.digest != captured.manifest_digest
            || manifest.total_bytes != captured.total_bytes
        {
            return Err(StoreError::Malformed(
                "the captured checkpoint manifest does not match its sealed archive".into(),
            ));
        }

        let mut launch = self
            .launches
            .get_launch(&origin.launch_id)
            .await
            .map_err(workspace_store_error)?
            .ok_or_else(|| StoreError::Malformed("the Trigger Launch no longer exists".into()))?;
        validate_launch_workspace_authority(&launch, owner, origin)?;
        let workspace = launch
            .workspace
            .as_ref()
            .expect("authority validated above");
        if workspace.checkpoint_intent.as_ref() != Some(intent) {
            if let Some(receipt) = reconciled_recovery_pair_receipt(workspace, intent) {
                return Ok(receipt);
            }
            return Err(StoreError::Malformed(
                "the lifecycle checkpoint intent changed before commit".into(),
            ));
        }
        let mut checkpoint = WorkspaceCheckpoint {
            id: intent.id.clone(),
            launch_id: launch.id.clone(),
            version: intent.version,
            workspace_epoch: origin.workspace_epoch,
            session_id: intent.session_id.clone(),
            runtime_generation,
            object_reference: Self::checkpoint_intent_key(owner, &launch.id, intent),
            archive_sha256: captured.archive_sha256.clone(),
            manifest,
            integrity: CheckpointIntegrity::Complete,
            recovery_pair: None,
            created_at: now.to_string(),
            delete_after: Some(workspace.delete_after.clone()),
        };
        let reference = TriggerRecoveryPairRef::seal(
            &checkpoint,
            workspace.baseline.digest.clone(),
            Self::recovery_conversation_intent_key(owner, &launch.id, intent),
            conversation,
        )
        .map_err(|error| StoreError::Malformed(error.to_string()))?;
        checkpoint.recovery_pair = Some(reference.clone());
        let stored_workspace =
            CheckpointObjectStore::put_checkpoint(self, owner, &checkpoint, &captured.archive)
                .await
                .map_err(workspace_store_error)?;
        if stored_workspace != checkpoint.object_reference {
            return Err(StoreError::Malformed(
                "workspace checkpoint storage returned another immutable reference".into(),
            ));
        }
        let stored_conversation = self
            .put_recovery_conversation(owner, &checkpoint, conversation)
            .await
            .map_err(workspace_store_error)?;
        if stored_conversation != reference.conversation_object_reference {
            return Err(StoreError::Malformed(
                "conversation checkpoint storage returned another immutable reference".into(),
            ));
        }
        launch
            .complete_lifecycle_checkpoint(intent, Some(checkpoint), now)
            .map_err(|error| StoreError::Malformed(error.to_string()))?;
        let committed = match replace_unleased_launch(self.launches.as_ref(), launch).await {
            Ok(committed) => committed,
            Err(error) => {
                if let Some(receipt) = self
                    .reconcile_recovery_pair_commit(owner, origin, intent)
                    .await?
                {
                    return Ok(receipt);
                }
                // The immutable object may already exist. Retain the exact
                // intent so the caller retries this destination; clearing it
                // here would make terminal cleanup unable to find the object.
                return Err(error);
            }
        };
        if committed
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.retired_checkpoint.as_ref())
            .is_some()
        {
            if let Err(error) = self.settle_retired_checkpoint(owner, committed).await {
                tracing::warn!(
                    launch_id = %origin.launch_id,
                    %error,
                    "workspace checkpoint committed with durable object-deletion debt"
                );
            }
        }
        Ok(TriggerRecoveryPairCommitReceipt {
            storage: WorkspaceCheckpointCommitOutcome::Committed,
            reference,
        })
    }

    async fn fail_checkpoint(
        &self,
        owner: &SubjectId,
        origin: &TriggerSessionOrigin,
        intent: &WorkspaceCheckpointIntent,
        failure: WorkspaceCheckpointFailure,
        recovery_required: bool,
    ) -> Result<(), StoreError> {
        let mut launch = self
            .launches
            .get_launch(&origin.launch_id)
            .await
            .map_err(workspace_store_error)?
            .ok_or_else(|| StoreError::Malformed("the Trigger Launch no longer exists".into()))?;
        validate_launch_workspace_authority(&launch, owner, origin)?;
        launch
            .fail_lifecycle_checkpoint(intent, failure, recovery_required)
            .map_err(|error| StoreError::Malformed(error.to_string()))?;
        replace_unleased_launch(self.launches.as_ref(), launch).await?;
        Ok(())
    }

    async fn mark_checkpoint_recovery_required(
        &self,
        owner: &SubjectId,
        origin: &TriggerSessionOrigin,
        reference: &TriggerRecoveryPairRef,
        failure: WorkspaceCheckpointFailure,
    ) -> Result<(), StoreError> {
        for attempt in 0..RECOVERY_REQUIRED_CAS_RETRIES {
            let mut launch = self
                .launches
                .get_launch(&origin.launch_id)
                .await
                .map_err(workspace_store_error)?
                .ok_or_else(|| {
                    StoreError::Malformed("the Trigger Launch no longer exists".into())
                })?;
            validate_launch_workspace_authority(&launch, owner, origin)?;
            if recovery_pair_is_fenced(&launch, reference) {
                return Ok(());
            }
            launch
                .mark_committed_checkpoint_recovery_required(reference, failure.clone())
                .map_err(|error| StoreError::Malformed(error.to_string()))?;
            let expected = launch.revision_number;
            launch.revision_number = expected
                .checked_add(1)
                .ok_or_else(|| StoreError::Malformed("the Launch revision is exhausted".into()))?;
            let error = match self.launches.replace_launch(expected, None, &launch).await {
                Ok(_) => return Ok(()),
                Err(
                    error @ (TriggerStoreError::RevisionConflict { .. }
                    | TriggerStoreError::Unavailable(_)),
                ) => error,
                Err(error) => return Err(workspace_store_error(error)),
            };

            match self.launches.get_launch(&origin.launch_id).await {
                Ok(Some(observed)) => {
                    validate_launch_workspace_authority(&observed, owner, origin)?;
                    if recovery_pair_is_fenced(&observed, reference) {
                        return Ok(());
                    }
                }
                Ok(None) => {
                    return Err(StoreError::Malformed(
                        "the Trigger Launch no longer exists".into(),
                    ));
                }
                Err(read_error) if attempt + 1 == RECOVERY_REQUIRED_CAS_RETRIES => {
                    return Err(workspace_store_error(read_error));
                }
                Err(_) => {}
            }
            if attempt + 1 == RECOVERY_REQUIRED_CAS_RETRIES {
                return Err(workspace_store_error(error));
            }
        }
        unreachable!("the bounded recovery-required CAS loop always returns")
    }
}

impl ChronoStorageTriggerWorkspaceStore {
    async fn reconcile_recovery_pair_commit(
        &self,
        owner: &SubjectId,
        origin: &TriggerSessionOrigin,
        intent: &WorkspaceCheckpointIntent,
    ) -> Result<Option<TriggerRecoveryPairCommitReceipt>, StoreError> {
        let launch = self
            .launches
            .get_launch(&origin.launch_id)
            .await
            .map_err(workspace_store_error)?
            .ok_or_else(|| StoreError::Malformed("the Trigger Launch no longer exists".into()))?;
        validate_launch_workspace_authority(&launch, owner, origin)?;
        Ok(launch
            .workspace
            .as_ref()
            .and_then(|workspace| reconciled_recovery_pair_receipt(workspace, intent)))
    }

    /// Delete only an object already made obsolete by a durable Launch commit.
    ///
    /// The debt is cleared in a second CAS. If either operation loses its
    /// response, the retained checkpoint record makes the exact deletion
    /// idempotently discoverable on the next reservation or cleanup pass.
    async fn settle_retired_checkpoint(
        &self,
        owner: &SubjectId,
        mut launch: layer2_core::Launch,
    ) -> Result<layer2_core::Launch, StoreError> {
        let retired = launch
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.retired_checkpoint.clone())
            .ok_or_else(|| {
                StoreError::Malformed("the Launch has no retired checkpoint deletion debt".into())
            })?;
        CheckpointObjectStore::delete_checkpoint(self, owner, &retired)
            .await
            .map_err(workspace_store_error)?;
        let workspace = launch
            .workspace
            .as_mut()
            .expect("retired checkpoint exists");
        if workspace.retired_checkpoint.as_ref() != Some(&retired) {
            return Err(StoreError::Malformed(
                "the retired checkpoint changed before its debt was cleared".into(),
            ));
        }
        workspace.retired_checkpoint = None;
        replace_unleased_launch(self.launches.as_ref(), launch).await
    }
}

#[cfg(test)]
fn reconciled_checkpoint_outcome(
    workspace: &layer2_core::WorkspaceRecord,
    intent: &WorkspaceCheckpointIntent,
) -> Option<WorkspaceCheckpointCommitOutcome> {
    if workspace.checkpoint_intent.as_ref() == Some(intent)
        || workspace.checkpoint_version < intent.version
    {
        return None;
    }
    let object_was_committed = [
        workspace.retired_checkpoint.as_ref(),
        workspace.previous_checkpoint.as_ref(),
        workspace.latest_checkpoint.as_ref(),
        workspace.sealed_contribution_checkpoint.as_ref(),
    ]
    .into_iter()
    .flatten()
    .any(|checkpoint| checkpoint.id == intent.id && checkpoint.version == intent.version);
    Some(if object_was_committed {
        WorkspaceCheckpointCommitOutcome::Committed
    } else {
        WorkspaceCheckpointCommitOutcome::Unchanged
    })
}

fn reconciled_recovery_pair_receipt(
    workspace: &layer2_core::WorkspaceRecord,
    intent: &WorkspaceCheckpointIntent,
) -> Option<TriggerRecoveryPairCommitReceipt> {
    if workspace.checkpoint_intent.as_ref() == Some(intent)
        || workspace.checkpoint_version < intent.version
    {
        return None;
    }
    [
        workspace.retired_checkpoint.as_ref(),
        workspace.previous_checkpoint.as_ref(),
        workspace.latest_checkpoint.as_ref(),
        workspace.sealed_contribution_checkpoint.as_ref(),
    ]
    .into_iter()
    .flatten()
    .find(|checkpoint| checkpoint.id == intent.id && checkpoint.version == intent.version)
    .and_then(|checkpoint| checkpoint.recovery_pair.clone())
    .map(|reference| TriggerRecoveryPairCommitReceipt {
        storage: WorkspaceCheckpointCommitOutcome::Committed,
        reference,
    })
}

fn recovery_pair_is_fenced(
    launch: &layer2_core::Launch,
    reference: &TriggerRecoveryPairRef,
) -> bool {
    let Some(workspace) = launch.workspace.as_ref() else {
        return false;
    };
    let Some(checkpoint) = workspace.latest_checkpoint.as_ref() else {
        return false;
    };
    workspace.checkpoint_intent.is_none()
        && workspace.lifecycle_checkpoint.state == LifecycleCheckpointState::RecoveryRequired
        && workspace.lifecycle_checkpoint.next_due_at.is_none()
        && workspace
            .lifecycle_checkpoint
            .last_failure
            .as_ref()
            .is_some_and(|failure| failure.retry_at.is_none())
        && checkpoint.recovery_pair.as_ref() == Some(reference)
        && reference.validate_checkpoint(checkpoint).is_ok()
}

async fn replace_unleased_launch(
    store: &dyn LaunchStore,
    mut launch: layer2_core::Launch,
) -> Result<layer2_core::Launch, StoreError> {
    let expected = launch.revision_number;
    launch.revision_number = expected
        .checked_add(1)
        .ok_or_else(|| StoreError::Malformed("the Launch revision is exhausted".into()))?;
    store
        .replace_launch(expected, None, &launch)
        .await
        .map_err(workspace_store_error)
}

fn workspace_store_error(error: TriggerStoreError) -> StoreError {
    match error {
        TriggerStoreError::Unavailable(detail) => StoreError::Unavailable(detail),
        other => StoreError::Malformed(other.to_string()),
    }
}

fn select_recovery_checkpoint<'a>(
    launch: &'a layer2_core::Launch,
    owner: &SubjectId,
    origin: &TriggerSessionOrigin,
) -> Result<Option<&'a WorkspaceCheckpoint>, StoreError> {
    validate_launch_workspace_authority(launch, owner, origin)?;
    let workspace = launch
        .workspace
        .as_ref()
        .expect("authority validated above");
    let Some(checkpoint) = workspace.latest_checkpoint.as_ref() else {
        return Ok(None);
    };
    checkpoint
        .validate_complete(&launch.id)
        .map_err(|error| StoreError::Malformed(error.to_string()))?;
    let expected_session = origin
        .session_id()
        .map_err(|error| StoreError::Malformed(error.to_string()))?;
    if checkpoint.workspace_epoch != origin.workspace_epoch
        || checkpoint.session_id != expected_session
        || checkpoint.manifest.source_revision != origin.source_revision
    {
        return Err(StoreError::Malformed(
            "the latest checkpoint does not belong to this Trigger session".into(),
        ));
    }
    Ok(Some(checkpoint))
}

fn validate_launch_workspace_authority(
    launch: &layer2_core::Launch,
    owner: &SubjectId,
    origin: &TriggerSessionOrigin,
) -> Result<(), StoreError> {
    let workspace = launch.workspace.as_ref().ok_or_else(|| {
        StoreError::Malformed("the Trigger Launch has no workspace authority".into())
    })?;
    if &launch.owner != owner
        || launch.revision_digest != origin.revision_digest
        || launch.source_sha != origin.source_revision
        || workspace.workspace_epoch != origin.workspace_epoch
        || workspace.baseline.source_revision != origin.source_revision
        || workspace.baseline.digest != origin.manifest_digest
        || workspace.baseline_object_reference != origin.object_reference
        || workspace.baseline_archive_sha256 != origin.archive_sha256
    {
        return Err(StoreError::Malformed(
            "the Trigger Launch and session workspace authorities disagree".into(),
        ));
    }
    Ok(())
}

fn require_sha256_value(value: &str, field: &str) -> Result<(), TriggerStoreError> {
    if value.len() != 64
        || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        || value.bytes().any(|byte| byte.is_ascii_uppercase())
    {
        return Err(TriggerStoreError::Malformed(format!("{field} is invalid")));
    }
    Ok(())
}

fn require_sha256(value: &str) -> Result<(), TriggerStoreError> {
    require_sha256_value(value, "workspace archive SHA-256")
}

fn require_git_sha(value: &str, field: &str) -> Result<(), TriggerStoreError> {
    if value.len() != 40
        || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        || value.bytes().any(|byte| byte.is_ascii_uppercase())
    {
        return Err(TriggerStoreError::Malformed(format!(
            "{field} is not a canonical Git SHA-1"
        )));
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn workspace_archive_limit() -> u64 {
    MAX_REPOSITORY_BYTES.saturating_add(ARCHIVE_OVERHEAD_BYTES)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashMap,
        sync::{
            atomic::{AtomicBool, Ordering},
            Mutex,
        },
    };

    use axum::{
        body::Bytes,
        extract::{Query, State},
        http::{HeaderMap, StatusCode},
        response::{IntoResponse, Response},
        routing::{get, post},
        Router,
    };
    use layer2_core::{
        CheckpointId, CheckpointIntegrity, Launch, LaunchQuery, LifecycleCheckpointState,
        SessionId, SessionLink, TriggerPage, TriggerRevisionId, WorkerLease,
        WorkspaceCheckpointPurpose, WorkspaceRecord,
    };

    #[derive(Debug, Clone)]
    struct StoredObject {
        content_type: String,
        bytes: Vec<u8>,
    }

    #[derive(Clone, Default)]
    struct StorageFixture {
        objects: Arc<tokio::sync::Mutex<HashMap<String, StoredObject>>>,
        fail_upload_containing: Arc<tokio::sync::Mutex<Option<String>>>,
        fail_delete_containing: Arc<tokio::sync::Mutex<Option<String>>>,
        retain_delete_containing: Arc<tokio::sync::Mutex<Option<String>>>,
    }

    #[derive(Deserialize)]
    struct ObjectQuery {
        key: String,
        #[serde(rename = "contentType")]
        content_type: Option<String>,
    }

    async fn fixture_upload(
        State(fixture): State<StorageFixture>,
        Query(query): Query<ObjectQuery>,
        headers: HeaderMap,
        body: Bytes,
    ) -> StatusCode {
        assert_eq!(
            headers
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer tok")
        );
        let mut failure = fixture.fail_upload_containing.lock().await;
        if failure
            .as_ref()
            .is_some_and(|needle| query.key.contains(needle))
        {
            *failure = None;
            return StatusCode::SERVICE_UNAVAILABLE;
        }
        drop(failure);
        fixture.objects.lock().await.insert(
            query.key,
            StoredObject {
                content_type: query.content_type.expect("upload content type"),
                bytes: body.to_vec(),
            },
        );
        StatusCode::OK
    }

    async fn fixture_download(
        State(fixture): State<StorageFixture>,
        Query(query): Query<ObjectQuery>,
    ) -> Response {
        match fixture.objects.lock().await.get(&query.key).cloned() {
            Some(object) => (StatusCode::OK, object.bytes).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }

    async fn fixture_delete(
        State(fixture): State<StorageFixture>,
        Query(query): Query<ObjectQuery>,
    ) -> StatusCode {
        let mut failure = fixture.fail_delete_containing.lock().await;
        if failure
            .as_ref()
            .is_some_and(|needle| query.key.contains(needle))
        {
            *failure = None;
            return StatusCode::SERVICE_UNAVAILABLE;
        }
        drop(failure);
        let mut retained = fixture.retain_delete_containing.lock().await;
        if retained
            .as_ref()
            .is_some_and(|needle| query.key.contains(needle))
        {
            *retained = None;
            return StatusCode::OK;
        }
        drop(retained);
        fixture.objects.lock().await.remove(&query.key);
        StatusCode::OK
    }

    async fn storage_fixture() -> (String, StorageFixture, tokio::task::JoinHandle<()>) {
        let fixture = StorageFixture::default();
        let app = Router::new()
            .route(
                "/api/buckets/bkt/objects",
                post(fixture_upload).delete(fixture_delete),
            )
            .route("/api/buckets/bkt/objects/download", get(fixture_download))
            .with_state(fixture.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind storage fixture");
        let address = listener.local_addr().expect("storage fixture address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve storage fixture");
        });
        (format!("http://{address}"), fixture, server)
    }

    struct TestLaunchStore {
        launch: Mutex<Launch>,
        hide_next_replace: AtomicBool,
        conflict_next_replace: AtomicBool,
    }

    impl TestLaunchStore {
        fn new(launch: Launch) -> Self {
            Self {
                launch: Mutex::new(launch),
                hide_next_replace: AtomicBool::new(false),
                conflict_next_replace: AtomicBool::new(false),
            }
        }

        fn current(&self) -> Launch {
            self.launch.lock().unwrap().clone()
        }

        fn hide_next_replace(&self) {
            self.hide_next_replace.store(true, Ordering::SeqCst);
        }

        fn conflict_next_replace(&self) {
            self.conflict_next_replace.store(true, Ordering::SeqCst);
        }
    }

    #[async_trait::async_trait]
    impl LaunchStore for TestLaunchStore {
        async fn insert_or_get_current(
            &self,
            launch: &Launch,
        ) -> Result<Launch, TriggerStoreError> {
            Ok(launch.clone())
        }

        async fn get_current_launch(
            &self,
            owner: &SubjectId,
            revision: &TriggerRevisionId,
        ) -> Result<Option<Launch>, TriggerStoreError> {
            let launch = self.current();
            Ok((&launch.owner == owner && &launch.revision.id == revision).then_some(launch))
        }

        async fn insert_new(&self, _launch: &Launch) -> Result<(), TriggerStoreError> {
            Err(TriggerStoreError::AlreadyExists)
        }

        async fn expire_current_and_insert(
            &self,
            _expired: &Launch,
            replacement: &Launch,
        ) -> Result<Launch, TriggerStoreError> {
            *self.launch.lock().unwrap() = replacement.clone();
            Ok(replacement.clone())
        }

        async fn get_launch(&self, id: &LaunchId) -> Result<Option<Launch>, TriggerStoreError> {
            let launch = self.current();
            Ok((launch.id == *id).then_some(launch))
        }

        async fn list_launches(
            &self,
            _query: &LaunchQuery,
        ) -> Result<TriggerPage<Launch>, TriggerStoreError> {
            Ok(TriggerPage {
                items: vec![self.current()],
                next: None,
            })
        }

        async fn replace_launch(
            &self,
            expected_revision: u64,
            _expected_fence: Option<u64>,
            launch: &Launch,
        ) -> Result<Launch, TriggerStoreError> {
            let mut stored = self.launch.lock().unwrap();
            if stored.revision_number != expected_revision {
                return Err(TriggerStoreError::RevisionConflict {
                    expected: expected_revision,
                    actual: stored.revision_number,
                });
            }
            if self.conflict_next_replace.swap(false, Ordering::SeqCst) {
                stored.revision_number = stored
                    .revision_number
                    .checked_add(1)
                    .expect("test Launch revision advances");
                return Err(TriggerStoreError::RevisionConflict {
                    expected: expected_revision,
                    actual: stored.revision_number,
                });
            }
            *stored = launch.clone();
            if self.hide_next_replace.swap(false, Ordering::SeqCst) {
                return Err(TriggerStoreError::Unavailable(
                    "injected committed Launch CAS response loss".into(),
                ));
            }
            Ok(launch.clone())
        }

        async fn acquire_lease(
            &self,
            _id: &LaunchId,
            _expected_revision: u64,
            _holder: &str,
            _acquired_at: &str,
            _expires_at: &str,
        ) -> Result<Option<Launch>, TriggerStoreError> {
            Err(TriggerStoreError::Unavailable(
                "the test store does not lease Launch workers".into(),
            ))
        }

        async fn release_lease(
            &self,
            _id: &LaunchId,
            _expected_revision: u64,
            _lease: &WorkerLease,
        ) -> Result<Launch, TriggerStoreError> {
            Err(TriggerStoreError::Unavailable(
                "the test store does not lease Launch workers".into(),
            ))
        }
    }

    fn recovery_fixture() -> (
        Launch,
        TriggerSessionOrigin,
        WorkspaceCheckpointIntent,
        layer3_adapter::WorkspaceCheckpointPackage,
        Vec<u8>,
    ) {
        let mut launch = crate::trigger::worker::tests::launch_fixture();
        launch.state = layer2_core::LaunchState::Ready;
        let baseline = WorkspaceManifest::seal(launch.source_sha.clone(), Vec::new()).unwrap();
        let session_id = layer2_core::trigger_session_id(&launch.id).unwrap();
        launch.session = Some(SessionLink {
            session_id: session_id.clone(),
            environment_profile_id: "envp_recovery_pair".into(),
            runtime_generation: 1,
            created_at: "2026-08-27T00:00:00Z".into(),
        });
        let origin = TriggerSessionOrigin {
            launch_id: launch.id.clone(),
            revision_digest: launch.revision_digest.clone(),
            sandbox_profile: launch.revision.content.session.sandbox_profile.clone(),
            github_service_id: launch.connection.as_ref().unwrap().service_id.clone(),
            github_catalog_service_id: launch
                .connection
                .as_ref()
                .unwrap()
                .github_catalog_service_id
                .clone(),
            github_user_service_slug: launch
                .connection
                .as_ref()
                .unwrap()
                .github_user_service_slug
                .clone(),
            workspace_epoch: 1,
            source_revision: launch.source_sha.clone(),
            manifest_digest: baseline.digest.clone(),
            object_reference: ChronoStorageTriggerWorkspaceStore::baseline_key(
                &launch.owner,
                &launch.id,
            ),
            archive_sha256: "a".repeat(64),
            timeout_seconds: launch.effective.timeout_seconds,
            pricing_policy_reference: layer2_core::CMA_TRIGGER_ZERO_COST_POLICY_V1.into(),
            billing_mode: layer2_core::BillingMode::ZeroCost,
            pricing: None,
        };
        origin.validate().unwrap();
        launch.workspace = Some(WorkspaceRecord {
            baseline: baseline.clone(),
            baseline_object_reference: origin.object_reference.clone(),
            baseline_archive_sha256: origin.archive_sha256.clone(),
            latest_checkpoint: None,
            previous_checkpoint: None,
            retired_checkpoint: None,
            sealed_contribution_checkpoint: None,
            checkpoint_intent: None,
            lifecycle_checkpoint: Default::default(),
            checkpoint_version: 0,
            workspace_epoch: origin.workspace_epoch,
            retention_policy_id: "trigger-test-retention".into(),
            delete_after: "2026-09-27T00:00:00Z".into(),
        });
        let intent = match launch
            .reserve_lifecycle_checkpoint(
                WorkspaceCheckpointPurpose::PreTeardown,
                &session_id,
                &launch.owner.clone(),
                1,
                "2026-08-27T00:01:00Z",
            )
            .unwrap()
        {
            WorkspaceCheckpointReservation::Capture(intent) => intent,
            other => panic!("expected a lifecycle capture reservation, got {other:?}"),
        };
        let archive = b"exact workspace archive at the conversation boundary".to_vec();
        let captured = layer3_adapter::WorkspaceCheckpointPackage {
            epoch: origin.workspace_epoch,
            version: intent.version,
            source_revision: origin.source_revision.clone(),
            manifest_digest: baseline.digest,
            archive_sha256: digest(&archive),
            entries: Vec::new(),
            total_bytes: 0,
            quiesced_writers: 1,
            writer_state: layer3_adapter::WorkspaceWriterState::HeldForTeardown,
            writer_resume_token: Some("resume-pair-1".into()),
            includes_git_metadata: false,
            archive,
        };
        (
            launch,
            origin,
            intent,
            captured,
            b"opaque exact conversation at the workspace boundary".to_vec(),
        )
    }

    fn recovery_store(
        base_url: String,
        launches: Arc<TestLaunchStore>,
    ) -> ChronoStorageTriggerWorkspaceStore {
        ChronoStorageTriggerWorkspaceStore::new(
            base_url,
            "bkt",
            Arc::new(crate::store::token::StaticToken("tok".into())),
            launches,
        )
    }

    #[test]
    fn baseline_keys_are_subject_and_launch_scoped_without_exposing_subject_ids() {
        let alice = SubjectId("alice@example.test".into());
        let bob = SubjectId("bob@example.test".into());
        let launch = LaunchId("lnch_1".into());
        let alice_key = ChronoStorageTriggerWorkspaceStore::baseline_key(&alice, &launch);
        let bob_key = ChronoStorageTriggerWorkspaceStore::baseline_key(&bob, &launch);

        assert_ne!(alice_key, bob_key);
        assert!(!alice_key.contains(&alice.0));
        assert!(alice_key.ends_with("/launches/lnch_1/baseline-v1.pack"));
        assert!(
            ChronoStorageTriggerWorkspaceStore::require_reference(&bob, &launch, &alice_key)
                .is_err()
        );
    }

    #[tokio::test]
    async fn baseline_import_keeps_gitlinks_out_of_the_provider_blob_inventory() {
        let (base_url, fixture, server) = storage_fixture().await;
        let launch = crate::trigger::worker::tests::launch_fixture();
        let owner = launch.owner.clone();
        let launch_id = launch.id.clone();
        let source_revision = launch.source_sha.clone();
        let store = recovery_store(base_url, Arc::new(TestLaunchStore::new(launch)));
        let gitmodules = br#"[submodule "vendor/library"]
	path = vendor/library
	url = ../../shared/library.git
"#;
        let mut object = sha1::Sha1::new();
        object.update(format!("blob {}\0", gitmodules.len()).as_bytes());
        object.update(gitmodules);
        let gitmodules_id = hex(&object.finalize());
        let blob = BaselineImportBlob::new(&gitmodules_id, gitmodules.len() as u64).unwrap();
        let chunk = BaselineImportChunk {
            provider_object_id: gitmodules_id.clone(),
            ordinal: 0,
            bytes: gitmodules.len() as u64,
            sha256: digest(gitmodules),
        };
        let source = crate::trigger::github::WorkspaceSourcePlan {
            source_revision: source_revision.clone(),
            tree_digest: "f".repeat(64),
            entries: vec![
                crate::trigger::github::WorkspaceSourceEntry {
                    path: ".gitmodules".into(),
                    mode: 0o100644,
                    provider_object_id: gitmodules_id.clone(),
                    bytes: gitmodules.len() as u64,
                },
                crate::trigger::github::WorkspaceSourceEntry {
                    path: "vendor/library".into(),
                    mode: 0o160000,
                    provider_object_id: "b".repeat(40),
                    bytes: 0,
                },
            ],
            blobs: vec![blob.clone()],
        };
        let metadata = crate::trigger::github::workspace_blob_metadata(
            &blob,
            gitmodules,
            vec![chunk.clone()],
            false,
            true,
        )
        .unwrap();
        let (manifest, plan) = crate::trigger::github::build_baseline_archive_plan(
            &source,
            &BTreeMap::from([(gitmodules_id.clone(), metadata)]),
        )
        .unwrap();

        store
            .begin_baseline_import(
                &owner,
                &launch_id,
                &source_revision,
                &source.tree_digest,
                &source.blobs,
            )
            .await
            .unwrap();
        store
            .put_baseline_import_chunk(
                &owner,
                &launch_id,
                &source_revision,
                &blob,
                &chunk,
                gitmodules,
            )
            .await
            .unwrap();
        let outcome = store
            .finalize_baseline_import(&owner, &launch_id, &manifest, plan)
            .await
            .expect("a gitlink is metadata rather than a missing provider blob");

        let gitlink = outcome
            .manifest
            .entries
            .iter()
            .find(|entry| entry.kind == WorkspaceEntryKind::Submodule)
            .unwrap();
        assert_eq!(
            gitlink.provider_object_id.as_deref(),
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        );
        let retained = fixture.objects.lock().await;
        let baseline = retained
            .get(&outcome.object_reference)
            .expect("the complete baseline archive is durable");
        assert!(baseline
            .bytes
            .windows(b"../../shared/library.git".len())
            .any(|window| window == b"../../shared/library.git"));
        assert_eq!(source.blobs.len(), 1);
        assert_eq!(source.blobs[0].provider_object_id, gitmodules_id);
        drop(retained);
        server.abort();
    }

    #[test]
    fn checkpoint_keys_are_subject_launch_and_version_scoped_and_strictly_rechecked() {
        let alice = SubjectId("alice@example.test".into());
        let bob = SubjectId("bob@example.test".into());
        let launch = LaunchId("lnch_1".into());
        let mut checkpoint = WorkspaceCheckpoint {
            id: CheckpointId("../opaque-checkpoint-id".into()),
            launch_id: launch,
            version: 4,
            workspace_epoch: 2,
            session_id: SessionId("sesn_1".into()),
            runtime_generation: 3,
            object_reference: String::new(),
            archive_sha256: "a".repeat(64),
            manifest: WorkspaceManifest::seal("b".repeat(40), Vec::new()).unwrap(),
            integrity: CheckpointIntegrity::Complete,
            recovery_pair: None,
            created_at: "2026-08-27T00:00:00Z".into(),
            delete_after: Some("2026-09-27T00:00:00Z".into()),
        };
        let alice_key = ChronoStorageTriggerWorkspaceStore::checkpoint_key(&alice, &checkpoint);
        let bob_key = ChronoStorageTriggerWorkspaceStore::checkpoint_key(&bob, &checkpoint);

        assert_ne!(alice_key, bob_key);
        assert!(!alice_key.contains(&alice.0));
        assert!(!alice_key.contains(&checkpoint.id.0));
        assert!(alice_key.contains("/launches/lnch_1/checkpoints/v4-"));
        let mut different_content = checkpoint.clone();
        different_content.archive_sha256 = "c".repeat(64);
        assert_eq!(
            alice_key,
            ChronoStorageTriggerWorkspaceStore::checkpoint_key(&alice, &different_content)
        );
        checkpoint.object_reference = alice_key.clone();
        assert_eq!(
            ChronoStorageTriggerWorkspaceStore::require_checkpoint_reference(&alice, &checkpoint)
                .unwrap(),
            alice_key
        );
        assert!(
            ChronoStorageTriggerWorkspaceStore::require_checkpoint_reference(&bob, &checkpoint)
                .is_err()
        );

        let conversation = b"opaque checkpoint-specific conversation";
        let alice_conversation =
            ChronoStorageTriggerWorkspaceStore::recovery_conversation_key(&alice, &checkpoint);
        let bob_conversation =
            ChronoStorageTriggerWorkspaceStore::recovery_conversation_key(&bob, &checkpoint);
        assert_ne!(alice_conversation, bob_conversation);
        assert_ne!(alice_conversation, alice_key);
        assert!(!alice_conversation.contains(&alice.0));
        assert!(!alice_conversation.contains(&checkpoint.id.0));
        assert!(!alice_conversation.contains("context.jsonl"));
        assert!(alice_conversation.ends_with(".conversation.snapshot"));
        let pair = TriggerRecoveryPairRef::seal(
            &checkpoint,
            "d".repeat(64),
            alice_conversation.clone(),
            conversation,
        )
        .unwrap();
        assert_eq!(
            ChronoStorageTriggerWorkspaceStore::require_recovery_conversation_reference(
                &alice,
                &checkpoint,
                &pair,
            )
            .unwrap(),
            alice_conversation
        );
        assert!(
            ChronoStorageTriggerWorkspaceStore::require_recovery_conversation_reference(
                &bob,
                &checkpoint,
                &pair,
            )
            .is_err()
        );
    }

    #[test]
    fn diff_keys_are_subject_launch_artifact_and_content_scoped() {
        let alice = SubjectId("alice@example.test".into());
        let bob = SubjectId("bob@example.test".into());
        let launch = LaunchId("lnch_1".into());
        let other_launch = LaunchId("lnch_2".into());
        let artifact = DiffArtifactId("../hostile-artifact-id".into());
        let sha256 = "a".repeat(64);
        let alice_key =
            ChronoStorageTriggerWorkspaceStore::diff_key(&alice, &launch, &artifact, &sha256);

        assert_ne!(
            alice_key,
            ChronoStorageTriggerWorkspaceStore::diff_key(&bob, &launch, &artifact, &sha256)
        );
        assert_ne!(
            alice_key,
            ChronoStorageTriggerWorkspaceStore::diff_key(&alice, &other_launch, &artifact, &sha256,)
        );
        assert_ne!(
            alice_key,
            ChronoStorageTriggerWorkspaceStore::diff_key(
                &alice,
                &launch,
                &artifact,
                &"b".repeat(64),
            )
        );
        assert!(!alice_key.contains(&alice.0));
        assert!(!alice_key.contains(&artifact.0));
        assert!(alice_key.contains("/launches/lnch_1/diffs/"));
        assert!(alice_key.ends_with(&format!("-{sha256}.json")));
    }

    #[test]
    fn recovery_keeps_the_latest_completed_checkpoint_across_newer_incomplete_intents() {
        let mut launch = crate::trigger::worker::tests::launch_fixture();
        let baseline = WorkspaceManifest::seal(launch.source_sha.clone(), Vec::new()).unwrap();
        let owner = launch.owner.clone();
        let origin = TriggerSessionOrigin {
            launch_id: launch.id.clone(),
            revision_digest: launch.revision_digest.clone(),
            sandbox_profile: launch.revision.content.session.sandbox_profile.clone(),
            github_service_id: launch.connection.as_ref().unwrap().service_id.clone(),
            github_catalog_service_id: launch
                .connection
                .as_ref()
                .unwrap()
                .github_catalog_service_id
                .clone(),
            github_user_service_slug: launch
                .connection
                .as_ref()
                .unwrap()
                .github_user_service_slug
                .clone(),
            workspace_epoch: 1,
            source_revision: launch.source_sha.clone(),
            manifest_digest: baseline.digest.clone(),
            object_reference: "trigger-test/baseline.pack".into(),
            archive_sha256: "a".repeat(64),
            timeout_seconds: launch.effective.timeout_seconds,
            pricing_policy_reference: layer2_core::CMA_TRIGGER_ZERO_COST_POLICY_V1.into(),
            billing_mode: layer2_core::BillingMode::ZeroCost,
            pricing: None,
        };
        origin.validate().unwrap();
        launch.workspace = Some(WorkspaceRecord {
            baseline,
            baseline_object_reference: origin.object_reference.clone(),
            baseline_archive_sha256: origin.archive_sha256.clone(),
            latest_checkpoint: None,
            previous_checkpoint: None,
            retired_checkpoint: None,
            sealed_contribution_checkpoint: None,
            checkpoint_intent: None,
            lifecycle_checkpoint: Default::default(),
            checkpoint_version: 0,
            workspace_epoch: origin.workspace_epoch,
            retention_policy_id: "trigger-test-retention".into(),
            delete_after: "2026-09-27T00:00:00Z".into(),
        });
        assert!(select_recovery_checkpoint(&launch, &owner, &origin)
            .unwrap()
            .is_none());

        let checkpoint = WorkspaceCheckpoint {
            id: CheckpointId("checkpoint-1".into()),
            launch_id: launch.id.clone(),
            version: 1,
            workspace_epoch: origin.workspace_epoch,
            session_id: origin.session_id().unwrap(),
            runtime_generation: 1,
            object_reference: "trigger-test/checkpoint-v1.pack".into(),
            archive_sha256: "b".repeat(64),
            manifest: WorkspaceManifest::seal(launch.source_sha.clone(), Vec::new()).unwrap(),
            integrity: CheckpointIntegrity::Complete,
            recovery_pair: None,
            created_at: "2026-08-27T00:00:00Z".into(),
            delete_after: Some("2026-09-27T00:00:00Z".into()),
        };
        launch.workspace.as_mut().unwrap().latest_checkpoint = Some(checkpoint.clone());
        assert_eq!(
            select_recovery_checkpoint(&launch, &owner, &origin).unwrap(),
            Some(&checkpoint)
        );
        assert!(
            select_recovery_checkpoint(&launch, &SubjectId("another-owner".into()), &origin)
                .is_err()
        );

        let intent = WorkspaceCheckpointIntent {
            id: CheckpointId("checkpoint-2".into()),
            version: 2,
            purpose: WorkspaceCheckpointPurpose::Periodic,
            session_id: origin.session_id().unwrap(),
            runtime_generation: 1,
            workspace_epoch: origin.workspace_epoch,
            requested_at: "2026-08-27T00:01:00Z".into(),
            retry_at: None,
        };
        launch.workspace.as_mut().unwrap().checkpoint_intent = Some(intent.clone());
        assert_eq!(
            select_recovery_checkpoint(&launch, &owner, &origin).unwrap(),
            Some(&checkpoint)
        );
        launch
            .workspace
            .as_mut()
            .unwrap()
            .lifecycle_checkpoint
            .state = LifecycleCheckpointState::RecoveryRequired;
        assert_eq!(
            select_recovery_checkpoint(&launch, &owner, &origin).unwrap(),
            Some(&checkpoint)
        );

        assert_eq!(
            reconciled_checkpoint_outcome(launch.workspace.as_ref().unwrap(), &intent),
            None,
            "an active intent is not evidence that its Launch CAS committed"
        );
        let mut committed = checkpoint.clone();
        committed.id = intent.id.clone();
        committed.version = intent.version;
        let workspace = launch.workspace.as_mut().unwrap();
        workspace.latest_checkpoint = Some(committed);
        workspace.checkpoint_version = intent.version;
        workspace.checkpoint_intent = None;
        assert_eq!(
            reconciled_checkpoint_outcome(workspace, &intent),
            Some(WorkspaceCheckpointCommitOutcome::Committed),
            "a lost CAS response reconciles from the exact retained object"
        );
        workspace.latest_checkpoint = None;
        assert_eq!(
            reconciled_checkpoint_outcome(workspace, &intent),
            Some(WorkspaceCheckpointCommitOutcome::Unchanged),
            "an unchanged high-water commit also reconciles without a second version"
        );
    }

    #[tokio::test]
    async fn recovery_pair_commit_retries_partial_second_write_and_hidden_launch_cas_success() {
        let (base_url, fixture, server) = storage_fixture().await;
        let (launch, origin, intent, captured, conversation) = recovery_fixture();
        let owner = launch.owner.clone();
        let launches = Arc::new(TestLaunchStore::new(launch));
        let store = recovery_store(base_url, Arc::clone(&launches));
        *fixture.fail_upload_containing.lock().await = Some(".conversation.snapshot".into());

        let first = store
            .commit_checkpoint(
                &owner,
                &origin,
                &intent,
                1,
                &captured,
                &conversation,
                "2026-08-27T00:01:00Z",
            )
            .await;
        assert!(matches!(first, Err(StoreError::Unavailable(_))));
        let workspace_key = ChronoStorageTriggerWorkspaceStore::checkpoint_intent_key(
            &owner,
            &origin.launch_id,
            &intent,
        );
        let conversation_key = ChronoStorageTriggerWorkspaceStore::recovery_conversation_intent_key(
            &owner,
            &origin.launch_id,
            &intent,
        );
        let objects = fixture.objects.lock().await;
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[&workspace_key].content_type, WORKSPACE_CONTENT_TYPE);
        assert!(!objects.contains_key(&conversation_key));
        drop(objects);
        assert!(launches
            .current()
            .workspace
            .unwrap()
            .checkpoint_intent
            .is_some());

        launches.hide_next_replace();
        let receipt = store
            .commit_checkpoint(
                &owner,
                &origin,
                &intent,
                1,
                &captured,
                &conversation,
                "2026-08-27T00:01:00Z",
            )
            .await
            .expect("a hidden committed Launch CAS reconciles from the exact checkpoint");
        assert_eq!(receipt.storage, WorkspaceCheckpointCommitOutcome::Committed);
        let objects = fixture.objects.lock().await;
        assert_eq!(objects.len(), 2);
        assert_eq!(
            objects[&conversation_key].content_type,
            RECOVERY_CONVERSATION_CONTENT_TYPE
        );
        drop(objects);

        let pair = store
            .resolve_recovery_pair(&owner, &origin, Some(&receipt.reference))
            .await
            .unwrap()
            .expect("both exact halves are recoverable after aggregate publication");
        assert_eq!(pair.workspace.archive, captured.archive);
        assert_eq!(pair.conversation, conversation);
        assert_eq!(pair.reference, receipt.reference);
        server.abort();
    }

    #[tokio::test]
    async fn recovery_pair_fence_retries_a_launch_revision_conflict() {
        let (base_url, _fixture, server) = storage_fixture().await;
        let (launch, origin, intent, captured, conversation) = recovery_fixture();
        let owner = launch.owner.clone();
        let launches = Arc::new(TestLaunchStore::new(launch));
        let store = recovery_store(base_url, Arc::clone(&launches));
        let receipt = store
            .commit_checkpoint(
                &owner,
                &origin,
                &intent,
                1,
                &captured,
                &conversation,
                "2026-08-27T00:01:00Z",
            )
            .await
            .unwrap();
        let committed_revision = launches.current().revision_number;
        launches.conflict_next_replace();

        store
            .mark_checkpoint_recovery_required(
                &owner,
                &origin,
                &receipt.reference,
                WorkspaceCheckpointFailure {
                    code: "workspace_writer_resume_unproven".into(),
                    detail: "The writer state could not be proven after commit.".into(),
                    observed_at: "2026-08-27T00:01:01Z".into(),
                    retry_at: None,
                },
            )
            .await
            .expect("a concurrent unrelated Launch revision is rebased");

        let retained = launches.current();
        assert_eq!(retained.revision_number, committed_revision + 2);
        assert!(recovery_pair_is_fenced(&retained, &receipt.reference));
        server.abort();
    }

    #[tokio::test]
    async fn recovery_pair_fence_reconciles_a_hidden_launch_cas_success() {
        let (base_url, _fixture, server) = storage_fixture().await;
        let (launch, origin, intent, captured, conversation) = recovery_fixture();
        let owner = launch.owner.clone();
        let launches = Arc::new(TestLaunchStore::new(launch));
        let store = recovery_store(base_url, Arc::clone(&launches));
        let receipt = store
            .commit_checkpoint(
                &owner,
                &origin,
                &intent,
                1,
                &captured,
                &conversation,
                "2026-08-27T00:01:00Z",
            )
            .await
            .unwrap();
        let committed_revision = launches.current().revision_number;
        launches.hide_next_replace();

        store
            .mark_checkpoint_recovery_required(
                &owner,
                &origin,
                &receipt.reference,
                WorkspaceCheckpointFailure {
                    code: "workspace_writer_resume_unproven".into(),
                    detail: "The writer state could not be proven after commit.".into(),
                    observed_at: "2026-08-27T00:01:01Z".into(),
                    retry_at: None,
                },
            )
            .await
            .expect("read-back proves the exact pair was durably fenced");

        let retained = launches.current();
        assert_eq!(retained.revision_number, committed_revision + 1);
        assert!(recovery_pair_is_fenced(&retained, &receipt.reference));
        server.abort();
    }

    #[tokio::test]
    async fn recovery_pair_commit_retries_a_failed_first_object_without_publication() {
        let (base_url, fixture, server) = storage_fixture().await;
        let (launch, origin, intent, captured, conversation) = recovery_fixture();
        let owner = launch.owner.clone();
        let launches = Arc::new(TestLaunchStore::new(launch));
        let store = recovery_store(base_url, Arc::clone(&launches));
        *fixture.fail_upload_containing.lock().await = Some(".pack".into());

        assert!(matches!(
            store
                .commit_checkpoint(
                    &owner,
                    &origin,
                    &intent,
                    1,
                    &captured,
                    &conversation,
                    "2026-08-27T00:01:00Z",
                )
                .await,
            Err(StoreError::Unavailable(_))
        ));
        assert!(fixture.objects.lock().await.is_empty());
        let retained = launches.current();
        assert!(retained.workspace.unwrap().checkpoint_intent.is_some());

        let receipt = store
            .commit_checkpoint(
                &owner,
                &origin,
                &intent,
                1,
                &captured,
                &conversation,
                "2026-08-27T00:01:00Z",
            )
            .await
            .expect("the same immutable pair destination remains retryable");
        assert_eq!(receipt.reference.checkpoint_id, intent.id);
        assert_eq!(fixture.objects.lock().await.len(), 2);
        server.abort();
    }

    #[tokio::test]
    async fn recovery_pair_resolution_refuses_stale_missing_corrupt_and_workspace_only_state() {
        let (base_url, fixture, server) = storage_fixture().await;
        let (launch, origin, intent, captured, conversation) = recovery_fixture();
        let owner = launch.owner.clone();
        let launches = Arc::new(TestLaunchStore::new(launch));
        let store = recovery_store(base_url, Arc::clone(&launches));
        let receipt = store
            .commit_checkpoint(
                &owner,
                &origin,
                &intent,
                1,
                &captured,
                &conversation,
                "2026-08-27T00:01:00Z",
            )
            .await
            .unwrap();
        let mut stale = receipt.reference.clone();
        stale.checkpoint_version += 1;
        assert!(matches!(
            store
                .resolve_recovery_pair(&owner, &origin, Some(&stale))
                .await,
            Err(StoreError::Malformed(_))
        ));

        let conversation_key = receipt.reference.conversation_object_reference.clone();
        let retained_conversation = fixture
            .objects
            .lock()
            .await
            .remove(&conversation_key)
            .unwrap();
        assert!(matches!(
            store.resolve_recovery_pair(&owner, &origin, None).await,
            Err(StoreError::Malformed(_))
        ));
        fixture.objects.lock().await.insert(
            conversation_key.clone(),
            StoredObject {
                content_type: RECOVERY_CONVERSATION_CONTENT_TYPE.into(),
                bytes: b"corrupt conversation".to_vec(),
            },
        );
        assert!(matches!(
            store.resolve_recovery_pair(&owner, &origin, None).await,
            Err(StoreError::Malformed(_))
        ));
        fixture
            .objects
            .lock()
            .await
            .insert(conversation_key, retained_conversation);

        let mut recovery_required = launches.current();
        recovery_required
            .workspace
            .as_mut()
            .unwrap()
            .lifecycle_checkpoint
            .state = LifecycleCheckpointState::RecoveryRequired;
        *launches.launch.lock().unwrap() = recovery_required;
        assert!(matches!(
            store.resolve_recovery_pair(&owner, &origin, None).await,
            Err(StoreError::Malformed(_))
        ));

        let mut legacy = launches.current();
        let workspace = legacy.workspace.as_mut().unwrap();
        workspace.lifecycle_checkpoint.state = LifecycleCheckpointState::Idle;
        let checkpoint = workspace.latest_checkpoint.as_mut().unwrap();
        checkpoint.recovery_pair = None;
        workspace.checkpoint_intent = None;
        *launches.launch.lock().unwrap() = legacy;
        assert!(store
            .resolve_recovery_pair(&owner, &origin, None)
            .await
            .unwrap()
            .is_none());
        server.abort();
    }

    #[tokio::test]
    async fn recovery_pair_deletion_debt_retries_both_immutable_objects() {
        let (base_url, fixture, server) = storage_fixture().await;
        let (launch, origin, intent, captured, conversation) = recovery_fixture();
        let owner = launch.owner.clone();
        let launches = Arc::new(TestLaunchStore::new(launch));
        let store = recovery_store(base_url, Arc::clone(&launches));
        let receipt = store
            .commit_checkpoint(
                &owner,
                &origin,
                &intent,
                1,
                &captured,
                &conversation,
                "2026-08-27T00:01:00Z",
            )
            .await
            .unwrap();
        let checkpoint = launches
            .current()
            .workspace
            .unwrap()
            .latest_checkpoint
            .unwrap();
        *fixture.fail_delete_containing.lock().await = Some(".conversation.snapshot".into());
        assert!(matches!(
            CheckpointObjectStore::delete_checkpoint(&store, &owner, &checkpoint).await,
            Err(TriggerStoreError::Unavailable(_))
        ));
        let objects = fixture.objects.lock().await;
        assert!(!objects.contains_key(&receipt.reference.workspace_object_reference));
        assert!(objects.contains_key(&receipt.reference.conversation_object_reference));
        drop(objects);

        CheckpointObjectStore::delete_checkpoint(&store, &owner, &checkpoint)
            .await
            .expect("the retained descriptor makes paired deletion retryable");
        assert!(fixture.objects.lock().await.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn acknowledged_but_retained_checkpoint_delete_remains_retryable() {
        let (base_url, fixture, server) = storage_fixture().await;
        let (launch, origin, intent, captured, conversation) = recovery_fixture();
        let owner = launch.owner.clone();
        let launches = Arc::new(TestLaunchStore::new(launch));
        let store = recovery_store(base_url, Arc::clone(&launches));
        let receipt = store
            .commit_checkpoint(
                &owner,
                &origin,
                &intent,
                1,
                &captured,
                &conversation,
                "2026-08-27T00:01:00Z",
            )
            .await
            .unwrap();
        let checkpoint = launches
            .current()
            .workspace
            .unwrap()
            .latest_checkpoint
            .unwrap();
        *fixture.retain_delete_containing.lock().await = Some(".pack".into());

        let first = CheckpointObjectStore::delete_checkpoint(&store, &owner, &checkpoint)
            .await
            .expect_err("a successful response is not proof that an object was deleted");
        assert!(matches!(first, TriggerStoreError::Unavailable(_)));
        let objects = fixture.objects.lock().await;
        assert!(objects.contains_key(&receipt.reference.workspace_object_reference));
        assert!(objects.contains_key(&receipt.reference.conversation_object_reference));
        drop(objects);

        CheckpointObjectStore::delete_checkpoint(&store, &owner, &checkpoint)
            .await
            .expect("the retained deletion debt converges on retry");
        assert!(fixture.objects.lock().await.is_empty());
        server.abort();
    }
}
