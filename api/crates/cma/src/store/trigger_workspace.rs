//! Launch-owned repository baselines and checkpoints in platform storage.

use std::sync::Arc;

use futures_util::StreamExt as _;
use layer2_core::{
    BaselineObjectStore, CheckpointObjectStore, EffectState, LaunchEffectKind, LaunchId,
    LaunchStore, SessionWorkspaceSource, StoreError, SubjectId, TriggerSessionOrigin,
    TriggerStoreError, WorkspaceCheckpoint, WorkspaceManifest, MAX_REPOSITORY_BYTES,
};
use reqwest::StatusCode;
use sha2::{Digest, Sha256};

use super::token::TokenSource;

const CONTENT_TYPE: &str = "application/vnd.chrono.cma-workspace.v1";
const ARCHIVE_OVERHEAD_BYTES: u64 = 256 * 1024 * 1024;

/// Repository artifacts reached through CMA's one storage machine identity.
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

    /// Immutable object key for one Launch checkpoint version. The opaque
    /// checkpoint ID is hashed so it cannot become path syntax.
    pub(crate) fn checkpoint_key(owner: &SubjectId, checkpoint: &WorkspaceCheckpoint) -> String {
        let owner_hash = hex(&Sha256::digest(owner.0.as_bytes()));
        let checkpoint_hash = hex(&Sha256::digest(checkpoint.id.0.as_bytes()));
        let content_hash = hex(&Sha256::digest(checkpoint.archive_sha256.as_bytes()));
        format!(
            "trigger-workspaces/subjects/{owner_hash}/launches/{}/checkpoints/v{}-{checkpoint_hash}-{content_hash}.pack",
            checkpoint.launch_id.0, checkpoint.version,
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

    async fn token(&self) -> Result<String, TriggerStoreError> {
        self.tokens
            .token()
            .await
            .map_err(|error| TriggerStoreError::Unavailable(format!("storage identity: {error}")))
    }

    async fn read_raw(&self, key: &str) -> Result<Option<Vec<u8>>, TriggerStoreError> {
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
        let max = MAX_REPOSITORY_BYTES.saturating_add(ARCHIVE_OVERHEAD_BYTES);
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

    async fn write_verified(
        &self,
        key: &str,
        archive_sha256: &str,
        archive: &[u8],
    ) -> Result<(), TriggerStoreError> {
        require_sha256(archive_sha256)?;
        if digest(archive) != archive_sha256 {
            return Err(TriggerStoreError::Malformed(
                "workspace archive does not match its SHA-256".into(),
            ));
        }
        let max = MAX_REPOSITORY_BYTES.saturating_add(ARCHIVE_OVERHEAD_BYTES);
        if u64::try_from(archive.len()).unwrap_or(u64::MAX) > max {
            return Err(TriggerStoreError::Malformed(
                "workspace archive exceeds the configured bound".into(),
            ));
        }
        if let Some(retained) = self.read_raw(key).await? {
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
            .query(&[("key", key), ("contentType", CONTENT_TYPE)])
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
        let retained = self.read_raw(key).await?.ok_or_else(|| {
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
}

#[async_trait::async_trait]
impl BaselineObjectStore for ChronoStorageTriggerWorkspaceStore {
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
        self.write_verified(&key, archive_sha256, archive).await?;
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
        let Some(bytes) = self.read_raw(&key).await? else {
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
        self.delete_raw(&key).await
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
        self.write_verified(&key, &checkpoint.archive_sha256, archive)
            .await?;
        Ok(key)
    }

    async fn get_checkpoint(
        &self,
        owner: &SubjectId,
        checkpoint: &WorkspaceCheckpoint,
    ) -> Result<Option<Vec<u8>>, TriggerStoreError> {
        let key = Self::require_checkpoint_reference(owner, checkpoint)?;
        let Some(bytes) = self.read_raw(&key).await? else {
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
        self.delete_raw(&key).await
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

    let checkpoint_effect = launch
        .effects
        .iter()
        .rev()
        .find(|effect| effect.kind == LaunchEffectKind::Checkpoint);
    if let Some(effect) = checkpoint_effect {
        match effect.state {
            EffectState::Queued
            | EffectState::Claimed
            | EffectState::Reconciling
            | EffectState::FailedRetryable => {
                return Err(StoreError::Unavailable(
                    "a newer workspace checkpoint is not durably committed yet".into(),
                ));
            }
            EffectState::RecoveryRequired | EffectState::FailedTerminal => {
                return Err(StoreError::Malformed(
                    "workspace recovery requires an explicit checkpoint resolution".into(),
                ));
            }
            EffectState::Succeeded if workspace.latest_checkpoint.is_none() => {
                return Err(StoreError::Malformed(
                    "the succeeded checkpoint effect has no complete checkpoint".into(),
                ));
            }
            EffectState::Succeeded | EffectState::Cancelled => {}
        }
    }

    let Some(checkpoint) = workspace.latest_checkpoint.as_ref() else {
        return Ok(None);
    };
    if checkpoint_effect.is_none_or(|effect| effect.state != EffectState::Succeeded) {
        return Err(StoreError::Malformed(
            "the latest checkpoint has no succeeded durable effect".into(),
        ));
    }
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

fn require_sha256(value: &str) -> Result<(), TriggerStoreError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(TriggerStoreError::Malformed(
            "workspace archive SHA-256 is invalid".into(),
        ));
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::{
        CheckpointId, CheckpointIntegrity, EffectRecord, LaunchEffectKind, SessionId,
        WorkspaceRecord,
    };

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
        assert_ne!(
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
    }

    #[test]
    fn recovery_selects_only_a_succeeded_latest_checkpoint_for_the_exact_session() {
        let mut launch = crate::trigger::worker::tests::launch_fixture();
        let baseline = WorkspaceManifest::seal(launch.source_sha.clone(), Vec::new()).unwrap();
        let owner = launch.owner.clone();
        let origin = TriggerSessionOrigin {
            launch_id: launch.id.clone(),
            revision_digest: launch.revision_digest.clone(),
            sandbox_profile: launch.revision.content.session.sandbox_profile.clone(),
            github_service_id: launch.connection.as_ref().unwrap().service_id.clone(),
            workspace_epoch: 1,
            source_revision: launch.source_sha.clone(),
            manifest_digest: baseline.digest.clone(),
            object_reference: "trigger-test/baseline.pack".into(),
            archive_sha256: "a".repeat(64),
        };
        origin.validate().unwrap();
        launch.workspace = Some(WorkspaceRecord {
            baseline,
            baseline_object_reference: origin.object_reference.clone(),
            baseline_archive_sha256: origin.archive_sha256.clone(),
            latest_checkpoint: None,
            previous_checkpoint: None,
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
            created_at: "2026-08-27T00:00:00Z".into(),
            delete_after: Some("2026-09-27T00:00:00Z".into()),
        };
        launch.workspace.as_mut().unwrap().latest_checkpoint = Some(checkpoint.clone());
        launch.effects.push(EffectRecord {
            kind: LaunchEffectKind::Checkpoint,
            state: EffectState::Succeeded,
            intent_key: "checkpoint-effect".into(),
            attempt: 1,
            reconciliation_attempt: 0,
            fence: 1,
            provider_request_id: None,
            external_resource_ids: Default::default(),
            next_attempt_at: None,
            last_failure: None,
            updated_at: "2026-08-27T00:00:00Z".into(),
        });
        assert_eq!(
            select_recovery_checkpoint(&launch, &owner, &origin).unwrap(),
            Some(&checkpoint)
        );
        assert!(
            select_recovery_checkpoint(&launch, &SubjectId("another-owner".into()), &origin)
                .is_err()
        );

        launch.effects.last_mut().unwrap().state = EffectState::Claimed;
        assert!(matches!(
            select_recovery_checkpoint(&launch, &owner, &origin),
            Err(StoreError::Unavailable(_))
        ));
        launch.effects.last_mut().unwrap().state = EffectState::RecoveryRequired;
        assert!(matches!(
            select_recovery_checkpoint(&launch, &owner, &origin),
            Err(StoreError::Malformed(_))
        ));
    }
}
