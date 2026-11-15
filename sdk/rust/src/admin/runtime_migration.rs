//! Exact custom-image guards and read-only activity observations.
use super::Administration;
use crate::{Result, instances::Sandbox, resources::identity};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

/// Immutable origin guards. This is not an admission basis or clearance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeMigrationOrigin {
    /// Sandbox identity.
    pub sandbox_id: String,
    /// Original owner.
    pub owner: String,
    /// Immutable profile identity.
    pub profile_id: String,
    /// Immutable profile revision.
    pub profile_revision: u32,
    /// Original profile content digest.
    pub profile_digest: String,
    /// Original custom image.
    pub profile_image: String,
    /// Exact retained home attachment; provider lineage remains opaque to the SDK.
    pub home: serde_json::Map<String, serde_json::Value>,
}

/// Exact immutable-image migration guards, checked against server-owned authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomRuntimeMigration {
    /// Stable admission request identity.
    pub request_key: String,
    /// Original Sandbox revision.
    pub expected_revision: u64,
    /// Original effective image.
    pub expected_image: String,
    /// Approved immutable target in the same repository.
    pub target_image: String,
    /// Immutable origin identity and retained home guards.
    pub origin: RuntimeMigrationOrigin,
}

/// Operator request; approval and activity evidence are resolved by the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomRuntimeMigrationRequest {
    /// Exact migration guards.
    pub migration: CustomRuntimeMigration,
    /// Reference to the separately installed operator authority.
    pub authority_id: String,
    /// Current Sandbox revision, including an explicit failed restart.
    pub expected_current_revision: u64,
    /// Missing means false. A failed restart requires fresh server clearance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_failed: Option<bool>,
}

/// Reasons an observation cannot establish inactivity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeActivityUnknownReason {
    /// The source is not qualified for this ID.
    UnqualifiedSource,
    /// The source context is unsupported.
    UnsupportedContext,
    /// No coherent source watermark is available.
    MissingWatermark,
    /// A stored authority is malformed.
    MalformedAuthority,
    /// Required source rows are missing or exceed the bound.
    IncompleteCoverage,
    /// The bounded observation registry is full.
    CardinalityExceeded,
    /// Source continuity changed.
    SourceChanged,
    /// The observation source is unavailable.
    Unavailable,
}

/// Content-free observation. Neither Unknown nor a reference grants permission.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RuntimeActivityStatus {
    /// No inactivity conclusion can be drawn.
    Unknown {
        /// Why the observation is inconclusive.
        reason: RuntimeActivityUnknownReason,
    },
    /// The observation contains activity.
    Active,
    /// A complete quiet interval has not yet been observed.
    Observing,
    /// The server has retained a proof, still subject to admission checks.
    Quiet {
        /// Opaque lookup handle, not bearer authorization.
        proof_reference: String,
    },
}

impl Administration<'_> {
    /// Request custom-image migration or exact accepted replay; requires control capability.
    pub async fn migrate_sandbox_runtime(
        &self,
        id: &str,
        body: &CustomRuntimeMigrationRequest,
    ) -> Result<Sandbox> {
        let sandbox: Sandbox = self
            .client
            .operation(
                Method::POST,
                &["admin", "sandboxes", id, "runtime-migration"],
                Some(body),
                StatusCode::ACCEPTED,
            )
            .await?;
        identity(&sandbox.id, id)?;
        Ok(sandbox)
    }

    /// Observe without admitting work; requires ReadAllSandboxes independently of control.
    pub async fn observe_sandbox_runtime_activity(
        &self,
        id: &str,
    ) -> Result<RuntimeActivityStatus> {
        self.client
            .get(
                &["admin", "sandboxes", id, "runtime-update", "activity"],
                &[],
            )
            .await
    }
}
