//! Provider-neutral CMA Trigger and Launch REST contract.

use std::collections::{BTreeMap, HashMap};

use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{Client, Error, Result};

/// An open JSON object used for versioned Trigger policy fragments.
pub type JsonObject = Map<String, Value>;

/// One bounded cursor page returned by Trigger collections.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CursorPage<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

/// Session resources expressed on the Trigger wire.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerSessionSize {
    pub cpu_millis: u32,
    pub memory_mib: u32,
}

/// Immutable publication availability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerAvailability {
    /// Available for new Launch creation.
    Published,
    /// Retained for old Launches but unavailable for new ones.
    Retired,
}

/// Repository visibility observed through the selected connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryVisibility {
    /// Public repository.
    Public,
    /// Private repository.
    Private,
    /// Provider-internal repository visibility.
    Internal,
}

/// How the upstream source revision is selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerSourceMode {
    /// Use the commit sealed into the Trigger revision.
    PinnedCommit,
    /// Resolve the configured branch head at explicit Start.
    BranchHeadAtLaunch,
}

/// Who pays for a Trigger Launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerBillingMode {
    /// Contributor pays under the deployment's policy.
    Contributor,
    /// No contributor charge.
    ZeroCost,
    /// Publisher or sponsor pays.
    Sponsored,
}

/// Side-effect-free public README Trigger preview.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerPreview {
    pub public_id: String,
    pub availability: TriggerAvailability,
    pub publisher_display_identity: String,
    pub repository_id: u64,
    pub repository_owner: String,
    pub repository_name: String,
    pub visibility: RepositoryVisibility,
    pub allow_forking: bool,
    pub public_summary: String,
    pub permission_ceiling: String,
    pub required_capabilities: Vec<String>,
    pub resource_default: TriggerSessionSize,
    pub resource_ceiling: TriggerSessionSize,
    pub timeout_default_seconds: u32,
    pub timeout_ceiling_seconds: u32,
    pub billing_mode: TriggerBillingMode,
    pub pricing_policy_reference: String,
    pub revision_digest: String,
}

/// Stable provider-neutral repository identity.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryIdentity {
    pub repository_id: u64,
    pub repository_node_id: String,
    pub owner_id: u64,
    pub owner_node_id: String,
    pub canonical_owner: String,
    pub canonical_name: String,
    pub visibility: RepositoryVisibility,
    pub allow_forking: bool,
    pub default_branch: String,
    pub selected_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_parent_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_source_id: Option<u64>,
}

/// Canonical repository identity resolved through one exact reusable connection.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerRepositorySelection {
    pub repository: RepositoryIdentity,
    pub source_sha: String,
}

/// Immutable upstream selection in a published Trigger revision.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamRevision {
    pub repository: RepositoryIdentity,
    pub source_mode: TriggerSourceMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_source_sha: Option<String>,
    pub contribution_base_ref: String,
}

/// Versioned task template in a Trigger revision.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerTaskContent {
    pub public_summary: String,
    pub prompt_template: String,
    pub renderer_version: u32,
    pub input_fields: Vec<JsonObject>,
}

/// Complete immutable Trigger revision content.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerRevisionContent {
    pub schema_version: u32,
    pub trigger_id: String,
    pub revision_number: u32,
    pub publisher_subject_id: String,
    pub publisher_display_identity: String,
    pub upstream: UpstreamRevision,
    pub task: TriggerTaskContent,
    pub session: JsonObject,
    pub repository_access: JsonObject,
    pub setup: JsonObject,
    pub contribution: JsonObject,
    pub commercial_policy: JsonObject,
    pub override_policy: JsonObject,
    pub public_projection: JsonObject,
}

/// Repository selection supplied while authoring a Trigger.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositorySelectionDraft {
    pub repository_id: u64,
    pub selected_ref: String,
    pub contribution_base_ref: String,
    pub source_mode: TriggerSourceMode,
}

/// Editable Trigger content supplied by a publisher.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerContentDraft {
    pub schema_version: u32,
    pub publisher_display_identity: String,
    pub repository: RepositorySelectionDraft,
    pub task: JsonObject,
    pub session: JsonObject,
    pub repository_access: JsonObject,
    pub setup: JsonObject,
    pub contribution: JsonObject,
    pub commercial_policy: JsonObject,
    pub override_policy: JsonObject,
    pub public_projection: JsonObject,
}

/// Mutable publisher draft under optimistic concurrency.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerDraft {
    pub id: String,
    pub version: u64,
    pub next_revision_number: u32,
    pub content: TriggerRevisionContent,
    pub created_at: String,
    pub updated_at: String,
}

/// Publisher authority sealed with a revision.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerPublisherAuthority {
    pub service_id: String,
    pub service_state_version: i64,
    pub account_id: u64,
    pub account_login: String,
    pub role: String,
    pub verified_at: String,
}

/// Immutable published Trigger revision.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerRevision {
    pub id: String,
    pub public_id: String,
    pub digest: String,
    pub availability: TriggerAvailability,
    pub content: TriggerRevisionContent,
    pub authority: TriggerPublisherAuthority,
    pub published_at: String,
    pub retired_at: Option<String>,
}

/// Reusable NyxID GitHub connection projected without credentials.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerConnection {
    pub service_id: String,
    pub state_version: i64,
    pub state: String,
    pub account_id: Option<u64>,
    pub account_node_id: Option<String>,
    pub account_login: Option<String>,
    pub avatar_url: Option<String>,
    pub granted_scopes: Vec<String>,
    pub consent_observed_at: Option<String>,
}

/// Browser authorization handoff returned by NyxID connection operations.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionAuthorization {
    pub service_id: String,
    pub authorization_url: String,
    pub attempt_nonce: String,
}

/// Durable Launch state.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchState {
    Draft,
    AuthRequired,
    Preflight,
    ForkPending,
    ForkReady,
    BranchPending,
    WorkspacePending,
    SandboxPending,
    SessionPending,
    PromptPending,
    Ready,
    Contributing,
    PrPending,
    PrOpen,
    Completed,
    FailedRetryable,
    FailedTerminal,
    Cancelled,
}

impl LaunchState {
    fn wire_name(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::AuthRequired => "auth_required",
            Self::Preflight => "preflight",
            Self::ForkPending => "fork_pending",
            Self::ForkReady => "fork_ready",
            Self::BranchPending => "branch_pending",
            Self::WorkspacePending => "workspace_pending",
            Self::SandboxPending => "sandbox_pending",
            Self::SessionPending => "session_pending",
            Self::PromptPending => "prompt_pending",
            Self::Ready => "ready",
            Self::Contributing => "contributing",
            Self::PrPending => "pr_pending",
            Self::PrOpen => "pr_open",
            Self::Completed => "completed",
            Self::FailedRetryable => "failed_retryable",
            Self::FailedTerminal => "failed_terminal",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Public Launch step state.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchStepStatus {
    Pending,
    Running,
    Completed,
    Blocked,
    Retryable,
    Failed,
    Cancelled,
}

/// One stable progress row shown to the contributor.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchStep {
    pub key: String,
    pub sequence: u32,
    pub status: LaunchStepStatus,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub attempt: u32,
    pub safe_detail: String,
    pub remediation: String,
    pub external_resource_ids: BTreeMap<String, String>,
    pub correlation_id: String,
}

/// Public-safe Launch failure.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchFailure {
    pub class: String,
    pub code: String,
    pub detail: String,
    pub remediation: String,
    pub correlation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<String>,
    pub observed_at: String,
}

/// Contributor-editable Launch values.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct LaunchOverrides {
    #[serde(default)]
    pub task_inputs: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_suffix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_size: Option<TriggerSessionSize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u32>,
    #[serde(default)]
    pub optional_checks: Vec<String>,
}

/// Exact effective configuration sealed for Start.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveConfiguration {
    pub rendered_prompt: String,
    pub branch_suffix: String,
    pub permission: String,
    pub resource_size: TriggerSessionSize,
    pub timeout_seconds: u32,
    pub optional_checks: Vec<String>,
    pub digest: String,
}

/// Reconciled contributor fork.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForkRecord {
    pub repository: RepositoryIdentity,
    pub reused: bool,
    pub source_revision: String,
    pub canonical_url: String,
    pub ready_at: String,
}

/// Launch-owned contribution branch.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerBranchRecord {
    pub name: String,
    pub expected_head: String,
    pub current_head: String,
    pub canonical_url: String,
    pub created_at: String,
}

/// Provider attribution used for the reviewed commit.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitIdentity {
    pub name: String,
    pub email: String,
}

/// Diff change kind.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffChangeKind {
    Added,
    Modified,
    Deleted,
    ModeChanged,
    SymlinkChanged,
}

/// One file in a sealed diff review.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiffFile {
    pub path: String,
    pub change: DiffChangeKind,
    pub binary: bool,
    pub old_mode: Option<u32>,
    pub new_mode: Option<u32>,
    pub old_sha256: Option<String>,
    pub new_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_provider_object_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_provider_object_id: Option<String>,
    pub old_bytes: u64,
    pub new_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    pub patch_complete: bool,
}

/// One optional validation check attached to a diff.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiffCheck {
    pub key: String,
    pub passed: bool,
    pub summary: String,
}

/// Immutable sealed diff artifact.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiffArtifact {
    pub id: String,
    pub checkpoint_id: String,
    pub baseline_digest: String,
    pub checkpoint_digest: String,
    pub expected_branch_head: String,
    pub observed_branch_head: String,
    pub changed_file_limit: u32,
    pub total_changed_files: u32,
    pub files: Vec<DiffFile>,
    pub changed_bytes: u64,
    pub digest: String,
    pub object_bytes: u64,
    pub author: CommitIdentity,
    pub committer: CommitIdentity,
    pub workflow_paths: Vec<String>,
    pub validation: String,
    pub checks: Vec<DiffCheck>,
    pub created_at: String,
    pub delete_after: String,
    pub download_url: String,
}

/// Guarded branch-publication record.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContributionRecord {
    pub id: String,
    pub diff_artifact_id: String,
    pub revision_digest: String,
    pub baseline_digest: String,
    pub checkpoint_digest: String,
    pub diff_digest: String,
    pub branch_name: String,
    pub state: String,
    pub expected_head: String,
    pub base_tree_id: Option<String>,
    pub tree_id: Option<String>,
    pub commit_id: Option<String>,
    pub resulting_head: Option<String>,
    pub object_ids: Vec<String>,
    pub author: CommitIdentity,
    pub committer: CommitIdentity,
    pub confirmation_snapshot_digest: String,
    pub confirmed_at: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Pull-request operation reviewed by the contributor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestOperation {
    /// Open a new pull request.
    Open,
    /// Update the Launch-owned existing pull request.
    Update,
}

/// Pull-request reconciliation state.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestState {
    Creating,
    Updating,
    Open,
    ClosedUnmerged,
    Merged,
    RecoveryRequired,
}

/// Provider mergeability observation.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mergeability {
    Computing,
    Mergeable,
    Conflicting,
}

/// Durable pull-request intent and provider observation.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PullRequestRecord {
    pub operation: PullRequestOperation,
    pub marker: String,
    pub upstream_repository_id: u64,
    pub upstream_repository_node_id: String,
    pub fork_repository_id: u64,
    pub fork_repository_node_id: String,
    pub base_ref: String,
    pub head_owner: String,
    pub head_branch: String,
    pub head_ref: String,
    pub expected_base_revision: String,
    pub expected_head_revision: String,
    pub previous_head_revision: Option<String>,
    pub target_number: Option<u64>,
    pub update_metadata: bool,
    pub expected_previous_title: Option<String>,
    pub expected_previous_body_digest: Option<String>,
    pub title: String,
    pub body: String,
    pub body_digest: String,
    pub intent_digest: String,
    pub provider_id: Option<u64>,
    pub provider_node_id: Option<String>,
    pub number: Option<u64>,
    pub canonical_url: Option<String>,
    pub state: PullRequestState,
    pub base_revision: Option<String>,
    pub head_revision: Option<String>,
    pub observed_title: Option<String>,
    pub observed_body_digest: Option<String>,
    pub mergeability: Mergeability,
    pub checks: String,
    pub confirmation_snapshot_digest: String,
    pub confirmed_at: String,
    pub created_at: String,
    pub updated_at: String,
    pub provider_created_at: Option<String>,
    pub provider_updated_at: Option<String>,
}

/// Side-effect-free pull-request proposal sealed for explicit confirmation.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PullRequestDraft {
    pub launch_id: String,
    pub operation: PullRequestOperation,
    pub marker: String,
    pub upstream_repository_id: u64,
    pub upstream_repository_node_id: String,
    pub fork_repository_id: u64,
    pub fork_repository_node_id: String,
    pub base_ref: String,
    pub head_owner: String,
    pub head_branch: String,
    pub head_ref: String,
    pub expected_base_revision: String,
    pub expected_head_revision: String,
    pub previous_head_revision: Option<String>,
    pub target_number: Option<u64>,
    pub target_provider_id: Option<u64>,
    pub target_provider_node_id: Option<String>,
    pub update_metadata: bool,
    pub expected_previous_title: Option<String>,
    pub expected_previous_body_digest: Option<String>,
    pub title: String,
    pub body: String,
    pub body_digest: String,
    pub intent_digest: String,
}

/// Public-safe restricted service-gateway reach status.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchReachStatus {
    pub service_id: String,
    pub state: String,
    pub policy_generation: u64,
    pub policy_fence: u64,
    pub runtime_audience: String,
    pub expires_at_epoch_seconds: i64,
    pub numeric_binding_observed_at: String,
    pub refreshed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revocation_requested_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revocation_effective_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_refusal: Option<Value>,
    pub tombstone_acknowledged: bool,
    pub agent_key_revocation_confirmed: bool,
}

/// Initial-prompt delivery state, including the recovery-required fence.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptDeliveryState {
    Queued,
    DispatchFenced,
    WriteConfirmed,
    ExecutionObserved,
    Completed,
    Failed,
    RecoveryRequired,
}

/// Public prompt delivery observation.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptDelivery {
    pub launch_id: String,
    pub session_id: String,
    pub revision_digest: String,
    pub origin_key: String,
    pub payload_hash: String,
    pub state: PromptDeliveryState,
    pub attempt: u32,
    pub dispatch_fence_sequence: Option<u64>,
    pub queued_at: String,
    pub updated_at: String,
}

/// Complete contributor-owned Launch snapshot.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Launch {
    pub id: String,
    pub trigger_public_id: String,
    pub revision_digest: String,
    pub source_sha: String,
    pub state: LaunchState,
    pub revision_number: u64,
    pub revision_content: TriggerRevisionContent,
    pub blocked_reason: Option<Value>,
    pub failure: Option<LaunchFailure>,
    pub overrides: LaunchOverrides,
    pub effective: EffectiveConfiguration,
    pub pricing: Option<JsonObject>,
    pub connection: Option<TriggerConnection>,
    pub workflow_paths: Vec<String>,
    pub workflow_risk_acknowledged: bool,
    pub steps: Vec<LaunchStep>,
    pub fork: Option<ForkRecord>,
    pub branch: Option<TriggerBranchRecord>,
    pub workspace: Option<JsonObject>,
    pub session: Option<JsonObject>,
    pub reach: Option<LaunchReachStatus>,
    pub delivery: Option<PromptDelivery>,
    pub contribution_checkpoint_version: Option<u64>,
    pub diff: Option<DiffArtifact>,
    pub contribution: Option<ContributionRecord>,
    pub pull_request: Option<PullRequestRecord>,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub terminal_at: Option<String>,
}

/// Closed public Launch event taxonomy.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LaunchEventKind {
    Created,
    DraftUpdated,
    ConnectionSelected,
    Started,
    StateChanged { from: LaunchState, to: LaunchState },
    StepChanged { step: LaunchStep },
    Blocked { reason: Value },
    Failure { failure: LaunchFailure },
    EffectReconciled { effect: String },
    DeliveryChanged { state: PromptDeliveryState },
    ReachChanged { reach: LaunchReachStatus },
    ConfirmationRecorded { action: String },
    Cancelled,
}

/// One replayable Launch progress event.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchEvent {
    pub sequence: u64,
    pub launch_id: String,
    pub occurred_at: String,
    pub correlation_id: String,
    pub kind: LaunchEventKind,
}

/// Bounded Launch progress replay response.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchProgress {
    pub launch_id: String,
    pub events: Vec<LaunchEvent>,
    pub next_cursor: u64,
}

/// Diff, contribution, and pull-request review projection.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiffReview {
    pub launch_id: String,
    pub revision_number: u64,
    pub state: LaunchState,
    pub source_sha: String,
    pub workflow_paths: Vec<String>,
    pub artifact: Option<DiffArtifact>,
    pub contribution: Option<ContributionRecord>,
    pub pull_request: Option<PullRequestRecord>,
}

/// First-page Launch filters.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LaunchListRequest {
    pub state: Option<LaunchState>,
    pub trigger_public_id: Option<String>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

/// Create-draft command body.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateTriggerRequest {
    pub selected_service_id: String,
    pub content: TriggerContentDraft,
}

/// Replace-draft command body.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateTriggerRequest {
    pub expected_version: u64,
    pub selected_service_id: String,
    pub content: TriggerContentDraft,
}

/// Publish-draft command body.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublishTriggerRequest {
    pub expected_version: u64,
    pub selected_service_id: String,
}

/// Begin-connection command body.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BeginConnectionRequest {
    pub intent_id: String,
}

/// Update-Launch-overrides command body.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateLaunchOverridesRequest {
    pub expected_revision: u64,
    pub overrides: LaunchOverrides,
}

/// Select-Launch-connection command body.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SelectLaunchConnectionRequest {
    pub expected_revision: u64,
    pub service_id: String,
}

/// Explicit Launch Start command body.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchStartRequest {
    pub expected_revision: u64,
    pub snapshot_digest: String,
    #[serde(default)]
    pub acknowledge_workflow_risk: bool,
}

/// Digest-bound confirmation used for diff, publication, and cancellation.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchConfirmationRequest {
    pub expected_revision: u64,
    pub snapshot_digest: String,
}

/// Side-effect-free pull-request proposal request.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PullRequestReviewRequest {
    pub operation: PullRequestOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

/// Explicit pull-request Open or Update confirmation.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PullRequestConfirmRequest {
    pub expected_revision: u64,
    pub snapshot_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TriggerConnections {
    connections: Vec<TriggerConnection>,
}

impl Client {
    /// Read one public Trigger preview without sending a configured credential.
    pub async fn read_trigger_preview(&self, public_id: &str) -> Result<TriggerPreview> {
        identifier(public_id, "public Trigger id")?;
        self.request_public_json(
            Method::GET,
            &["triggers", "public", public_id],
            &[],
            None,
            StatusCode::OK,
            true,
        )
        .await
    }

    /// Create a publisher Trigger draft.
    pub async fn create_trigger(
        &self,
        request: CreateTriggerRequest,
        idempotency_key: &str,
    ) -> Result<TriggerDraft> {
        self.command(
            Method::POST,
            &["triggers"],
            request,
            StatusCode::CREATED,
            idempotency_key,
        )
        .await
    }

    /// List one publisher Trigger-draft page.
    pub async fn list_triggers(
        &self,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<CursorPage<TriggerDraft>> {
        let query = page_query(limit, cursor)?;
        self.request_public_json(
            Method::GET,
            &["triggers"],
            &query,
            None,
            StatusCode::OK,
            false,
        )
        .await
    }

    /// Read one publisher Trigger draft.
    pub async fn read_trigger(&self, trigger_id: &str) -> Result<TriggerDraft> {
        identifier(trigger_id, "Trigger id")?;
        self.request_public_json(
            Method::GET,
            &["triggers", trigger_id],
            &[],
            None,
            StatusCode::OK,
            false,
        )
        .await
    }

    /// Replace one publisher Trigger draft under optimistic concurrency.
    pub async fn update_trigger(
        &self,
        trigger_id: &str,
        request: UpdateTriggerRequest,
        idempotency_key: &str,
    ) -> Result<TriggerDraft> {
        identifier(trigger_id, "Trigger id")?;
        self.command(
            Method::PUT,
            &["triggers", trigger_id],
            request,
            StatusCode::OK,
            idempotency_key,
        )
        .await
    }

    /// Publish one immutable Trigger revision.
    pub async fn publish_trigger(
        &self,
        trigger_id: &str,
        request: PublishTriggerRequest,
        idempotency_key: &str,
    ) -> Result<TriggerRevision> {
        identifier(trigger_id, "Trigger id")?;
        self.command(
            Method::POST,
            &["triggers", trigger_id, "publish"],
            request,
            StatusCode::CREATED,
            idempotency_key,
        )
        .await
    }

    /// List one immutable Trigger-revision page.
    pub async fn list_trigger_revisions(
        &self,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<CursorPage<TriggerRevision>> {
        let query = page_query(limit, cursor)?;
        self.request_public_json(
            Method::GET,
            &["trigger-revisions"],
            &query,
            None,
            StatusCode::OK,
            false,
        )
        .await
    }

    /// Read one immutable publisher Trigger revision.
    pub async fn read_trigger_revision(&self, revision_id: &str) -> Result<TriggerRevision> {
        identifier(revision_id, "Trigger revision id")?;
        self.request_public_json(
            Method::GET,
            &["trigger-revisions", revision_id],
            &[],
            None,
            StatusCode::OK,
            false,
        )
        .await
    }

    /// Retire one Trigger revision from new Launch creation.
    pub async fn retire_trigger_revision(
        &self,
        revision_id: &str,
        idempotency_key: &str,
    ) -> Result<TriggerRevision> {
        identifier(revision_id, "Trigger revision id")?;
        self.command(
            Method::POST,
            &["trigger-revisions", revision_id, "retire"],
            HashMap::<String, Value>::new(),
            StatusCode::OK,
            idempotency_key,
        )
        .await
    }

    /// List reusable NyxID GitHub connections for the current subject.
    pub async fn list_trigger_connections(&self) -> Result<Vec<TriggerConnection>> {
        let body: TriggerConnections = self
            .request_public_json(
                Method::GET,
                &["trigger-connections"],
                &[],
                None,
                StatusCode::OK,
                false,
            )
            .await?;
        Ok(body.connections)
    }

    /// Begin an explicit NyxID GitHub consent attempt.
    pub async fn begin_trigger_connection(
        &self,
        request: BeginConnectionRequest,
        idempotency_key: &str,
    ) -> Result<ConnectionAuthorization> {
        self.command(
            Method::POST,
            &["trigger-connections"],
            request,
            StatusCode::OK,
            idempotency_key,
        )
        .await
    }

    /// Reconnect one exact reusable NyxID service.
    pub async fn reconnect_trigger_connection(
        &self,
        service_id: &str,
        idempotency_key: &str,
    ) -> Result<ConnectionAuthorization> {
        identifier(service_id, "service id")?;
        self.command(
            Method::POST,
            &["trigger-connections", service_id, "reconnect"],
            HashMap::<String, Value>::new(),
            StatusCode::OK,
            idempotency_key,
        )
        .await
    }

    /// Revalidate one exact reusable NyxID service.
    pub async fn preflight_trigger_connection(
        &self,
        service_id: &str,
    ) -> Result<TriggerConnection> {
        identifier(service_id, "service id")?;
        self.request_public_json(
            Method::GET,
            &["trigger-connections", service_id, "preflight"],
            &[],
            None,
            StatusCode::OK,
            false,
        )
        .await
    }

    /// Resolve one human-facing owner/name through an exact reusable connection.
    pub async fn resolve_trigger_repository(
        &self,
        service_id: &str,
        owner: &str,
        name: &str,
    ) -> Result<TriggerRepositorySelection> {
        identifier(service_id, "service id")?;
        repository_locator(owner, "repository owner")?;
        repository_locator(name, "repository name")?;
        self.request_public_json(
            Method::GET,
            &["trigger-connections", service_id, "repositories", "resolve"],
            &[("owner", owner.to_owned()), ("name", name.to_owned())],
            None,
            StatusCode::OK,
            false,
        )
        .await
    }

    /// Create or resume this subject's current Launch for one revision.
    pub async fn create_or_resume_launch(
        &self,
        public_id: &str,
        idempotency_key: &str,
    ) -> Result<Launch> {
        identifier(public_id, "public Trigger id")?;
        self.command(
            Method::POST,
            &["triggers", "public", public_id, "launches"],
            HashMap::<String, Value>::new(),
            StatusCode::CREATED,
            idempotency_key,
        )
        .await
    }

    /// Create a separate Launch after explicitly accepting extra resources.
    pub async fn create_new_launch(
        &self,
        public_id: &str,
        idempotency_key: &str,
    ) -> Result<Launch> {
        identifier(public_id, "public Trigger id")?;
        self.command(
            Method::POST,
            &["triggers", "public", public_id, "launches", "new"],
            serde_json::json!({ "acknowledge_additional_resources": true }),
            StatusCode::CREATED,
            idempotency_key,
        )
        .await
    }

    /// List one bounded contributor Launch page.
    pub async fn list_launches(&self, page: LaunchListRequest) -> Result<CursorPage<Launch>> {
        let limit = page.limit.unwrap_or(50);
        if !(1..=200).contains(&limit) {
            return Err(Error::protocol(
                "Launch page limit must be from 1 through 200",
            ));
        }
        let mut query = Vec::new();
        if let Some(state) = page.state {
            query.push(("state", state.wire_name().to_owned()));
        }
        if let Some(public_id) = page.trigger_public_id {
            query.push(("trigger_public_id", public_id));
        }
        query.push(("limit", limit.to_string()));
        if let Some(cursor) = page.cursor {
            query.push(("cursor", cursor));
        }
        self.request_public_json(
            Method::GET,
            &["launches"],
            &query,
            None,
            StatusCode::OK,
            false,
        )
        .await
    }

    /// Read one owned Launch.
    pub async fn read_launch(&self, launch_id: &str) -> Result<Launch> {
        identifier(launch_id, "Launch id")?;
        self.request_public_json(
            Method::GET,
            &["launches", launch_id],
            &[],
            None,
            StatusCode::OK,
            false,
        )
        .await
    }

    /// Replace contributor-editable Launch values.
    pub async fn update_launch_overrides(
        &self,
        launch_id: &str,
        request: UpdateLaunchOverridesRequest,
        idempotency_key: &str,
    ) -> Result<Launch> {
        identifier(launch_id, "Launch id")?;
        self.command(
            Method::PATCH,
            &["launches", launch_id, "overrides"],
            request,
            StatusCode::OK,
            idempotency_key,
        )
        .await
    }

    /// Select one exact reusable connection for a Launch.
    pub async fn select_launch_connection(
        &self,
        launch_id: &str,
        request: SelectLaunchConnectionRequest,
        idempotency_key: &str,
    ) -> Result<Launch> {
        identifier(launch_id, "Launch id")?;
        self.command(
            Method::PUT,
            &["launches", launch_id, "connection"],
            request,
            StatusCode::OK,
            idempotency_key,
        )
        .await
    }

    /// Start the exact reviewed Launch snapshot.
    pub async fn start_launch(
        &self,
        launch_id: &str,
        request: LaunchStartRequest,
        idempotency_key: &str,
    ) -> Result<Launch> {
        identifier(launch_id, "Launch id")?;
        self.command(
            Method::POST,
            &["launches", launch_id, "start"],
            request,
            StatusCode::ACCEPTED,
            idempotency_key,
        )
        .await
    }

    /// Request one sealed workspace diff without publishing it.
    pub async fn request_launch_diff(
        &self,
        launch_id: &str,
        request: LaunchConfirmationRequest,
        idempotency_key: &str,
    ) -> Result<Launch> {
        identifier(launch_id, "Launch id")?;
        self.command(
            Method::POST,
            &["launches", launch_id, "contributions", "review"],
            request,
            StatusCode::ACCEPTED,
            idempotency_key,
        )
        .await
    }

    /// Read the current sealed diff and contribution status.
    pub async fn read_launch_diff(&self, launch_id: &str) -> Result<DiffReview> {
        identifier(launch_id, "Launch id")?;
        self.request_public_json(
            Method::GET,
            &["launches", launch_id, "diff"],
            &[],
            None,
            StatusCode::OK,
            false,
        )
        .await
    }

    /// Publish the exact reviewed diff to the Launch branch.
    pub async fn publish_launch_branch(
        &self,
        launch_id: &str,
        request: LaunchConfirmationRequest,
        idempotency_key: &str,
    ) -> Result<Launch> {
        identifier(launch_id, "Launch id")?;
        self.command(
            Method::POST,
            &["launches", launch_id, "contributions", "publish"],
            request,
            StatusCode::ACCEPTED,
            idempotency_key,
        )
        .await
    }

    /// Seal an Open or Update proposal without writing GitHub.
    pub async fn review_pull_request(
        &self,
        launch_id: &str,
        request: PullRequestReviewRequest,
    ) -> Result<PullRequestDraft> {
        identifier(launch_id, "Launch id")?;
        self.request_public_json(
            Method::POST,
            &["launches", launch_id, "pull-request", "review"],
            &[],
            Some(to_value(request, "pull-request review request")?),
            StatusCode::OK,
            false,
        )
        .await
    }

    /// Explicitly confirm the exact sealed Open or Update proposal.
    pub async fn confirm_pull_request(
        &self,
        launch_id: &str,
        operation: PullRequestOperation,
        request: PullRequestConfirmRequest,
        idempotency_key: &str,
    ) -> Result<Launch> {
        identifier(launch_id, "Launch id")?;
        let operation = match operation {
            PullRequestOperation::Open => "open",
            PullRequestOperation::Update => "update",
        };
        self.command(
            Method::POST,
            &["launches", launch_id, "pull-request", operation],
            request,
            StatusCode::ACCEPTED,
            idempotency_key,
        )
        .await
    }

    /// Cancel one Launch while retaining its durable history.
    pub async fn cancel_launch(
        &self,
        launch_id: &str,
        request: LaunchConfirmationRequest,
        idempotency_key: &str,
    ) -> Result<Launch> {
        identifier(launch_id, "Launch id")?;
        self.command(
            Method::POST,
            &["launches", launch_id, "cancel"],
            request,
            StatusCode::OK,
            idempotency_key,
        )
        .await
    }

    /// Replay durable Launch progress after an accepted cursor.
    pub async fn read_launch_progress(
        &self,
        launch_id: &str,
        after: u64,
        limit: usize,
    ) -> Result<LaunchProgress> {
        identifier(launch_id, "Launch id")?;
        if limit == 0 || limit > 1_000 {
            return Err(Error::protocol(
                "Launch progress limit must be from 1 through 1000",
            ));
        }
        self.request_public_json(
            Method::GET,
            &["launches", launch_id, "events"],
            &[("after", after.to_string()), ("limit", limit.to_string())],
            None,
            StatusCode::OK,
            false,
        )
        .await
    }

    /// Download one immutable complete diff artifact.
    pub async fn download_diff_artifact(
        &self,
        launch_id: &str,
        artifact_id: &str,
    ) -> Result<Vec<u8>> {
        identifier(launch_id, "Launch id")?;
        identifier(artifact_id, "diff artifact id")?;
        self.request_public_bytes(
            &[
                "launches",
                launch_id,
                "diff-artifacts",
                artifact_id,
                "download",
            ],
            StatusCode::OK,
        )
        .await
    }

    async fn command<T, R>(
        &self,
        method: Method,
        path: &[&str],
        body: T,
        expected: StatusCode,
        idempotency_key: &str,
    ) -> Result<R>
    where
        T: Serialize,
        R: for<'de> Deserialize<'de>,
    {
        self.request_public_command(
            method,
            path,
            to_value(body, "Trigger command")?,
            expected,
            idempotency_key,
        )
        .await
    }
}

fn identifier(value: &str, name: &str) -> Result<()> {
    if value.trim().is_empty() || value.contains('/') {
        return Err(Error::protocol(format!(
            "{name} must be a non-empty path identifier"
        )));
    }
    Ok(())
}

fn repository_locator(value: &str, name: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 100
        || matches!(value, "." | "..")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(Error::protocol(format!(
            "{name} must be one canonical owner or repository name"
        )));
    }
    Ok(())
}

fn page_query(limit: u32, cursor: Option<&str>) -> Result<Vec<(&'static str, String)>> {
    if !(1..=200).contains(&limit) {
        return Err(Error::protocol(
            "Trigger page limit must be from 1 through 200",
        ));
    }
    let mut query = vec![("limit", limit.to_string())];
    if let Some(cursor) = cursor {
        query.push(("cursor", cursor.to_owned()));
    }
    Ok(query)
}

fn to_value(value: impl Serialize, name: &str) -> Result<Value> {
    serde_json::to_value(value)
        .map_err(|error| Error::protocol(format!("could not encode {name}: {error}")))
}
