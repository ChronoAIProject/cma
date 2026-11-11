use super::Secret;
use serde::{Deserialize, Serialize};

/// CMAEG `AppIntegration` wire document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppIntegration {
    /// Wire field `app_id`.
    pub app_id: u64,
    /// Wire field `app_ref`.
    pub app_ref: String,
    /// Wire field `display_name`.
    pub display_name: String,
    /// Wire field `host`.
    pub host: String,
    /// Wire field `status`.
    pub status: String,
    /// Wire field `verification`: omitted, explicitly null, or a value.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::native_media_update::present"
    )]
    pub verification: Option<Option<AppVerification>>,
    /// Wire field `verification_credential`.
    pub verification_credential: CredentialStatus,
    /// Wire field `version`.
    pub version: u64,
    /// Wire field `webhook_credential`.
    pub webhook_credential: WebhookCredentialStatus,
    /// Wire field `webhook_url`.
    pub webhook_url: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::native_media_update::present"
    )]
    /// Wire field `retired_at`.
    pub retired_at: Option<Option<i64>>,
}

/// CMAEG `AppList` wire document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppList {
    /// Wire field `items`.
    pub items: Vec<AppIntegration>,
    /// Wire field `next_cursor`: omitted, explicitly null, or a value.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::native_media_update::present"
    )]
    pub next_cursor: Option<Option<String>>,
}

/// CMAEG `AppVerification` wire document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppVerification {
    /// Wire field `evidence_ref`.
    pub evidence_ref: String,
    /// Wire field `verified_at`.
    pub verified_at: i64,
    /// Wire field `verified_by`.
    pub verified_by: String,
}

/// CMAEG `BindRequest` wire document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindRequest {
    /// Wire field `app_ref`.
    pub app_ref: String,
    /// Wire field `installation_id`.
    pub installation_id: u64,
    /// Wire field `proof_ref`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `account_id`.
    pub account_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `repository_ids`.
    pub repository_ids: Option<Vec<u64>>,
}

/// CMAEG `Binding` wire document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    /// Wire field `account_id`.
    pub account_id: u64,
    /// Wire field `app_id`.
    pub app_id: u64,
    /// Wire field `app_ref`.
    pub app_ref: String,
    /// Wire field `authority_revision`.
    pub authority_revision: u64,
    /// Wire field `binding_id`.
    pub binding_id: String,
    /// Wire field `installation_id`.
    pub installation_id: u64,
    /// Wire field `repository_ids`.
    pub repository_ids: Vec<u64>,
    /// Wire field `status`.
    pub status: String,
    /// Wire field `verification_ref`.
    pub verification_ref: String,
    /// Wire field `verified_by`.
    pub verified_by: String,
    /// Wire field `verified_until`.
    pub verified_until: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `version`.
    pub version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `requested_repository_ids`.
    pub requested_repository_ids: Option<Vec<u64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `approved_repository_ids`.
    pub approved_repository_ids: Option<Vec<u64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `excluded_repository_ids`.
    pub excluded_repository_ids: Option<Vec<u64>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::native_media_update::present"
    )]
    /// Wire field `approval`.
    pub approval: Option<Option<BindingApproval>>,
}

/// CMAEG `CreateAppRequest` wire document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateAppRequest {
    /// Wire field `app_id`.
    pub app_id: u64,
    /// Wire field `display_name`.
    pub display_name: String,
    /// Write-only request credential; never included in integration responses.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::native_media_update::present"
    )]
    pub webhook_secret: Option<Option<Secret>>,
}

/// CMAEG `CredentialStatus` wire document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialStatus {
    /// Wire field `present`.
    pub present: bool,
    /// Wire field `version`: omitted, explicitly null, or a value.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::native_media_update::present"
    )]
    pub version: Option<Option<u64>>,
}

/// CMAEG `GitHubCapabilities` wire document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitHubCapabilities {
    /// Wire field `app_identity_verification`.
    pub app_identity_verification: bool,
    /// Wire field `automatic_reconciliation`.
    pub automatic_reconciliation: bool,
    /// Wire field `dynamic_app_registration`.
    pub dynamic_app_registration: bool,
    /// Wire field `native_app_webhook`.
    pub native_app_webhook: bool,
    /// Wire field `ownership_verification`.
    pub ownership_verification: bool,
    /// Wire field `store_ready`.
    pub store_ready: bool,
    /// Wire field `webhook_secret_rotation`.
    pub webhook_secret_rotation: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Wire field `durable_dispatch`.
    pub durable_dispatch: Option<bool>,
}

/// CMAEG `Receipt` wire document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    /// Wire field `action`: omitted, explicitly null, or a value.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::native_media_update::present"
    )]
    pub action: Option<Option<String>>,
    /// Wire field `app_ref`.
    pub app_ref: String,
    /// Wire field `authority_revision`.
    pub authority_revision: u64,
    /// Wire field `body_sha256`.
    pub body_sha256: String,
    /// Wire field `delivery_id`.
    pub delivery_id: String,
    /// Wire field `event`.
    pub event: String,
    /// Wire field `installation_id`: omitted, explicitly null, or a value.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::native_media_update::present"
    )]
    pub installation_id: Option<Option<u64>>,
    /// Wire field `raw_body_expires_at`.
    pub raw_body_expires_at: i64,
    /// Bounded receipt and duplicate-detection deadline; absent on older gateways.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::native_media_update::present"
    )]
    pub retained_until: Option<Option<i64>>,
    /// Wire field `receipt_id`.
    pub receipt_id: String,
    /// Wire field `received_at`.
    pub received_at: i64,
    /// Wire field `repository_id`: omitted, explicitly null, or a value.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::native_media_update::present"
    )]
    pub repository_id: Option<Option<u64>>,
    /// Wire field `status`.
    pub status: String,
    /// Wire field `task_ids`.
    pub task_ids: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::native_media_update::present"
    )]
    /// Wire field `reason`.
    pub reason: Option<Option<String>>,
    /// Wire field `dispatch`: per-channel durable dispatch progress.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch: Option<Vec<DispatchProgress>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// One channel's durable GitHub dispatch progress.
pub struct DispatchProgress {
    /// Channel identity selected from the owner's persisted definitions.
    pub channel_id: String,
    /// `pending`, `dispatched` or a definitive refusal state.
    pub status: String,
    /// Optional bounded refusal/recovery reason.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::native_media_update::present"
    )]
    pub reason: Option<Option<String>>,
    /// Committed ordinary routed admission ID, when available.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::native_media_update::present"
    )]
    pub admission_id: Option<Option<String>>,
}

/// CMAEG `Task` wire document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    /// Wire field `accepted_at`.
    pub accepted_at: i64,
    /// Wire field `channel_id`.
    pub channel_id: String,
    /// Wire field `event_id`.
    pub event_id: String,
    /// Wire field `record_id`.
    pub record_id: String,
    /// Wire field `response_id`: omitted, explicitly null, or a value.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::native_media_update::present"
    )]
    pub response_id: Option<Option<String>>,
    /// Wire field `result`: omitted, explicitly null, or a value.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::native_media_update::present"
    )]
    pub result: Option<Option<TaskResult>>,
    /// Wire field `retained_until`.
    pub retained_until: i64,
    /// Wire field `status`.
    pub status: String,
    /// Wire field `target_count`.
    pub target_count: u64,
    /// Wire field `task_id`.
    pub task_id: String,
}

/// CMAEG `TaskResult` wire document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskResult {
    /// Wire field `branch`: omitted, explicitly null, or a value.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::native_media_update::present"
    )]
    pub branch: Option<Option<String>>,
    /// Wire field `commit`: omitted, explicitly null, or a value.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::native_media_update::present"
    )]
    pub commit: Option<Option<String>>,
    /// Wire field `pull_request`: omitted, explicitly null, or a value.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::native_media_update::present"
    )]
    pub pull_request: Option<Option<String>>,
    /// Wire field `status`.
    pub status: TaskResultStatus,
}

/// CMAEG `TaskResultStatus` wire document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskResultStatus {
    /// Wire value `succeeded`.
    Succeeded,
    /// Wire value `no_change`.
    NoChange,
}

/// CMAEG `UpdateAppRequest` wire document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateAppRequest {
    /// Wire field `display_name`.
    pub display_name: String,
    /// Wire field `expected_version`.
    pub expected_version: u64,
}

/// CMAEG `VerifyAppRequest` wire document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyAppRequest {
    /// Wire field `evidence_ref`.
    pub evidence_ref: String,
    /// Wire field `expected_version`.
    pub expected_version: u64,
}

/// CMAEG `WebhookCredentialOverlap` wire document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookCredentialOverlap {
    /// Wire field `valid_until`.
    pub valid_until: i64,
    /// Wire field `version`.
    pub version: u64,
}

/// CMAEG `WebhookCredentialStatus` wire document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookCredentialStatus {
    /// Wire field `present`.
    pub present: bool,
    /// Wire field `previous`: omitted, explicitly null, or a value.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::native_media_update::present"
    )]
    pub previous: Option<Option<WebhookCredentialOverlap>>,
    /// Wire field `version`: omitted, explicitly null, or a value.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::native_media_update::present"
    )]
    pub version: Option<Option<u64>>,
}

/// CMAEG `WebhookSecretRequest` wire document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookSecretRequest {
    /// Wire field `expected_version`.
    pub expected_version: u64,
    /// Wire field `overlap_seconds`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::native_media_update::present"
    )]
    pub overlap_seconds: Option<u32>,
    /// Write-only request credential; never included in integration responses.
    pub webhook_secret: Secret,
}

/// Administrator approval audit for installation authority.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingApproval {
    /// Wire field `verified_by`.
    pub verified_by: String,
    /// Wire field `evidence_ref`.
    pub evidence_ref: String,
    /// Wire field `approved_at`.
    pub approved_at: i64,
    /// Wire field `authority_revision`.
    pub authority_revision: u64,
    /// Wire field `repository_ids`.
    pub repository_ids: Vec<u64>,
}

/// Explicit initial approval or re-approval; installation identity remains fixed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyBindingRequest {
    /// Wire field `expected_version`.
    pub expected_version: u64,
    /// Wire field `evidence_ref`.
    pub evidence_ref: String,
    /// Wire field `repository_ids`.
    pub repository_ids: Vec<u64>,
}

/// Terminal integration retirement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetireAppRequest {
    /// Wire field `expected_version`.
    pub expected_version: u64,
}
