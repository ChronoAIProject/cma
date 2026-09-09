//! A distinct disk restored from the exact snapshot admitted for offline export.

use serde::{Deserialize, Serialize};

use crate::{AdapterError, HomeSnapshotExportRequest, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeSnapshotViewReceipt {
    pub request_sha256: String,
    pub volume_reference: String,
    pub volume_generation: String,
}

impl HomeSnapshotViewReceipt {
    pub fn validate_for(&self, request: &HomeSnapshotExportRequest) -> Result<()> {
        if self.request_sha256 != request.computed_sha256()?
            || self.volume_reference != format!("cma-view-{}", &self.request_sha256[..52])
            || self.volume_reference == request.snapshot.home.receipt.storage_reference
            || self.volume_generation.is_empty()
            || self.volume_generation.len() > 253
            || self.volume_generation.trim() != self.volume_generation
            || self.volume_generation.chars().any(char::is_control)
        {
            return Err(AdapterError::Rejected(
                "snapshot export view does not match its source authority".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeSnapshotViewObservation {
    pub receipt: HomeSnapshotViewReceipt,
    /// None while waiting for a consumer or the storage controller. Once verified,
    /// this exact backing volume may be observed without retaining the snapshot.
    pub bound_volume: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeSnapshotViewInventoryEntry {
    pub request: HomeSnapshotExportRequest,
    pub receipt: HomeSnapshotViewReceipt,
}

#[async_trait::async_trait]
pub trait HomeSnapshotViewStore: Send + Sync {
    async fn view_inventory(&self) -> Result<Vec<HomeSnapshotViewInventoryEntry>>;
    /// Returns before binding. The exporter consumer can then be scheduled.
    async fn prepare_view(
        &self,
        request: &HomeSnapshotExportRequest,
    ) -> Result<HomeSnapshotViewObservation>;
    /// Never recreate a missing view; verify snapshot UID through first binding.
    async fn observe_view(
        &self,
        request: &HomeSnapshotExportRequest,
        previous: &HomeSnapshotViewObservation,
    ) -> Result<HomeSnapshotViewObservation>;
    async fn remove_view(
        &self,
        request: &HomeSnapshotExportRequest,
        receipt: Option<&HomeSnapshotViewReceipt>,
    ) -> Result<()>;
}
