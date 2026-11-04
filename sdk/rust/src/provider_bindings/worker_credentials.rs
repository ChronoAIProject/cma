//! Gateway machine-only worker credential exclusion; no raw key is transmitted.
use super::ProviderBindings;
use crate::Result;
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

/// One canonical lowercase SHA-256 digest, without a prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerCredentialDigest {
    /// Bare 64-character digest; the server rejects malformed values.
    pub sha256: String,
}
/// Successful current lookup disposition, never a fallback for unavailable authority.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerCredentialClassification {
    /// Present in current or retained former worker-key history.
    KnownWorker,
    /// Successful lookup found no worker-key history for this digest.
    Unknown,
}
/// Classification response without owner, binding or resource metadata.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerCredentialClassificationView {
    /// The completed lookup result.
    pub classification: WorkerCredentialClassification,
}
impl ProviderBindings<'_> {
    /// Requires the Gateway's service-account access bearer and both management
    /// capabilities. Ordinary owner, worker and browser credentials cannot call it.
    pub async fn classify_worker_credential(
        &self,
        body: &WorkerCredentialDigest,
    ) -> Result<WorkerCredentialClassificationView> {
        self.0
            .operation(
                Method::POST,
                &["cmaeg", "worker-credentials", "classify"],
                Some(body),
                StatusCode::OK,
            )
            .await
    }
}
