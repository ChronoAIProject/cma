//! Recorded authority for one offline export attempt and its streamed publication.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    AdapterError, HomeSnapshotExportReceipt, HomeSnapshotExportRequest,
    HomeSnapshotViewObservation, HomeSnapshotViewReceipt, Result,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeSnapshotExportJobRequest {
    pub export: HomeSnapshotExportRequest,
    pub view: HomeSnapshotViewReceipt,
    /// Allocate durably before scheduling; only cleanup may retire an attempt.
    pub attempt: u64,
    pub image: String,
}

impl HomeSnapshotExportJobRequest {
    pub fn validate(&self) -> Result<()> {
        self.view.validate_for(&self.export)?;
        if self.attempt == 0 {
            return Err(invalid());
        }
        Self::validate_image(&self.image)
    }

    pub fn validate_image(image: &str) -> Result<()> {
        let (repository, digest) = image.split_once("@sha256:").ok_or_else(invalid)?;
        if repository.is_empty()
            || repository.len() > 512
            || !repository
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'/' | b'-' | b'_' | b':'))
            || digest.len() != 64
            || !digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(invalid());
        }
        Ok(())
    }

    pub fn computed_sha256(&self) -> Result<String> {
        self.validate()?;
        let mut hash = Sha256::new();
        hash.update(b"cma-home-export-job-v1\0");
        hash.update(serde_json::to_vec(self).map_err(|_| invalid())?);
        Ok(format!("{:x}", hash.finalize()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeSnapshotExportJobReceipt {
    pub request_sha256: String,
    pub pod_reference: String,
    pub pod_generation: String,
}

impl HomeSnapshotExportJobReceipt {
    pub fn validate_for(&self, request: &HomeSnapshotExportJobRequest) -> Result<()> {
        if self.request_sha256 != request.computed_sha256()?
            || self.pod_reference != format!("cma-export-{}", &self.request_sha256[..48])
            || self.pod_generation.is_empty()
            || self.pod_generation.len() > 253
            || self.pod_generation.trim() != self.pod_generation
            || self.pod_generation.chars().any(char::is_control)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HomeSnapshotExportJobPhase {
    Pending,
    Exporting,
    Ready,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeSnapshotExportJobObservation {
    pub receipt: HomeSnapshotExportJobReceipt,
    pub view: HomeSnapshotViewObservation,
    pub phase: HomeSnapshotExportJobPhase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeSnapshotExportJobInventoryEntry {
    pub request: HomeSnapshotExportJobRequest,
    pub receipt: HomeSnapshotExportJobReceipt,
}

#[async_trait::async_trait]
pub trait HomeSnapshotExportJobStore: Send + Sync {
    async fn export_job_inventory(&self) -> Result<Vec<HomeSnapshotExportJobInventoryEntry>>;
    async fn prepare_export_job(
        &self,
        request: &HomeSnapshotExportJobRequest,
    ) -> Result<HomeSnapshotExportJobObservation>;
    async fn observe_export_job(
        &self,
        request: &HomeSnapshotExportJobRequest,
        previous: &HomeSnapshotExportJobObservation,
    ) -> Result<HomeSnapshotExportJobObservation>;
    /// Verify the exact Pod, bounded receipt, complete bytes and upload identity.
    async fn upload_export(
        &self,
        request: &HomeSnapshotExportJobRequest,
        ready: &HomeSnapshotExportJobObservation,
        store: &dyn crate::hosted::CheckpointArchiveStore,
    ) -> Result<HomeSnapshotExportReceipt>;
    async fn remove_export_job(
        &self,
        request: &HomeSnapshotExportJobRequest,
        receipt: Option<&HomeSnapshotExportJobReceipt>,
    ) -> Result<()>;
}

fn invalid() -> AdapterError {
    AdapterError::Rejected("snapshot export job authority is inconsistent".into())
}
