//! Storage snapshots name one immutable view of an exact persistent home.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{AdapterError, HomeVolumeAttachment, Result};

/// Persist before allocation. Retrying this request must not move the capture boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeSnapshotRequest {
    pub capture_id: String,
    pub epoch: u64,
    pub home: HomeVolumeAttachment,
    pub snapshot_class: String,
}

impl HomeSnapshotRequest {
    pub fn validate(&self) -> Result<()> {
        self.home.receipt.validate_for(&self.home.request)?;
        if self.epoch == 0 || !identifier(&self.capture_id) || !identifier(&self.snapshot_class) {
            return Err(invalid());
        }
        Ok(())
    }

    pub fn computed_sha256(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"cma-home-snapshot-v1\0");
        for value in [
            self.capture_id.as_str(),
            &self.home.request.computed_sha256(),
            &self.home.receipt.volume_id,
            &self.home.receipt.request_sha256,
            &self.home.receipt.storage_reference,
            &self.home.receipt.storage_generation,
            &self.snapshot_class,
        ] {
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value.as_bytes());
        }
        digest.update(self.epoch.to_be_bytes());
        format!("{:x}", digest.finalize())
    }
}

/// Record immediately after allocation, including when the background copy is pending.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeSnapshotReceipt {
    pub capture_id: String,
    pub request_sha256: String,
    pub snapshot_reference: String,
    pub snapshot_generation: String,
}

impl HomeSnapshotReceipt {
    pub fn validate_for(&self, request: &HomeSnapshotRequest) -> Result<()> {
        request.validate()?;
        if self.capture_id != request.capture_id
            || self.request_sha256 != request.computed_sha256()
            || !identifier(&self.snapshot_reference)
            || !identifier(&self.snapshot_generation)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeSnapshotObservation {
    pub receipt: HomeSnapshotReceipt,
    /// Provider capture time can precede completion of the background copy.
    pub captured_at: Option<String>,
    pub ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeSnapshotInventoryEntry {
    pub request: HomeSnapshotRequest,
    pub receipt: HomeSnapshotReceipt,
}

/// Allocation and observation never stop runtime writers or wait for a full copy.
#[async_trait::async_trait]
pub trait HomeSnapshotStore: Send + Sync {
    async fn snapshot_inventory(&self) -> Result<Vec<HomeSnapshotInventoryEntry>>;
    async fn prepare_snapshot(
        &self,
        request: &HomeSnapshotRequest,
    ) -> Result<HomeSnapshotObservation>;
    /// A recorded snapshot disappearing is a failure, never a request to recapture.
    async fn observe_snapshot(
        &self,
        request: &HomeSnapshotRequest,
        previous: &HomeSnapshotObservation,
    ) -> Result<HomeSnapshotObservation>;
    async fn remove_snapshot(
        &self,
        request: &HomeSnapshotRequest,
        receipt: Option<&HomeSnapshotReceipt>,
    ) -> Result<()>;
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn invalid() -> AdapterError {
    AdapterError::Rejected("home snapshot authority does not match its capture".into())
}
