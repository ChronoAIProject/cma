//! Launch-owned repository baselines and checkpoints in platform storage.

use std::sync::Arc;

use futures_util::StreamExt as _;
use layer2_core::{
    BaselineObjectStore, LaunchId, SessionWorkspaceSource, StoreError, SubjectId,
    TriggerSessionOrigin, TriggerStoreError, WorkspaceManifest, MAX_REPOSITORY_BYTES,
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
    http: reqwest::Client,
}

impl ChronoStorageTriggerWorkspaceStore {
    pub fn new(
        base_url: impl Into<String>,
        bucket: impl Into<String>,
        tokens: Arc<dyn TokenSource>,
    ) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            bucket: bucket.into(),
            tokens,
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
        let key = Self::baseline_key(owner, launch);
        let url = self.objects_url();
        let response = self
            .http
            .post(&url)
            .query(&[("key", key.as_str()), ("contentType", CONTENT_TYPE)])
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

        // A successful upload status is not integrity evidence. Read the exact
        // key back through the destination before the Launch may commit it.
        let retained = self.read_raw(&key).await?.ok_or_else(|| {
            TriggerStoreError::Unavailable(
                "workspace upload was acknowledged but no object was readable".into(),
            )
        })?;
        if digest(&retained) != archive_sha256 {
            return Err(TriggerStoreError::Malformed(
                "workspace destination did not retain the complete archive".into(),
            ));
        }
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
        let url = self.objects_url();
        let response = self
            .http
            .delete(&url)
            .query(&[("key", key.as_str())])
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
}
