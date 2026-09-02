//! Provider-neutral CMA Trigger and Launch REST contract.
//!
//! A [`Launch`] exposes server-owned [`LaunchPricingSnapshot`],
//! [`LaunchRuntimeAdmission`], [`LaunchQuotaSnapshot`], and
//! [`LaunchStartAdmission`] values. Clients must require the review snapshots
//! before Start and bind confirmation to the resulting effective digest. A
//! changed price, runtime, or quota requires a fresh saved review and explicit
//! reconfirmation; `draft_expires_at` is the server-owned pre-Start deadline.

use std::collections::{BTreeMap, HashMap};

use reqwest::{Method, StatusCode};
use serde::de::{self, Deserializer};
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

/// Current public preview availability after resolving repository policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerPreviewAvailability {
    /// The revision and current repository policy permit Launch review.
    Published,
    /// The publisher retired this immutable revision.
    Retired,
    /// Current repository identity or policy prevents Launch review.
    Unavailable,
}

/// Public-safe reason a published Trigger is currently unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerPreviewUnavailableReason {
    /// The current repository policy does not permit the public fork workflow.
    UpstreamNotForkable,
    /// The retained numeric upstream can no longer be read.
    UpstreamUnavailable,
    /// The retained upstream identity no longer matches the provider.
    UpstreamIdentityChanged,
    /// CMA could not safely resolve current repository policy.
    RepositoryPolicyUnavailable,
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

/// Runtime capability names accepted by the CMA Trigger contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerRequiredCapability {
    /// Interactive conversation input and output.
    Conversation,
    /// Conversation state survives supported runtime replacement.
    DurableConversation,
    /// The complete workspace can be captured and restored.
    WorkspaceCheckpoint,
    /// GitHub access crosses the restricted service gateway.
    RestrictedServiceGateway,
    /// Session-scoped credentials can be refreshed.
    CredentialRefresh,
    /// Session-scoped credentials can be revoked with evidence.
    CredentialRevocation,
}

/// Permission modes supported by the public Trigger MVP.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerPermissionMode {
    Guarded,
    Autonomous,
}

/// Public cost of the immutable Trigger defaults, before authenticated review.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerPreviewCost {
    ExactZero {
        currency: String,
        amount_micros: u64,
        display: String,
    },
    ContributorEstimate {
        currency: String,
        amount_micros: u64,
        display: String,
        quoted_at: String,
    },
    QuoteUnavailable,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum TriggerPreviewCostWire {
    ExactZero {
        currency: String,
        amount_micros: u64,
        display: String,
    },
    ContributorEstimate {
        currency: String,
        amount_micros: u64,
        display: String,
        quoted_at: String,
    },
    QuoteUnavailable,
}

impl<'de> Deserialize<'de> for TriggerPreviewCost {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match TriggerPreviewCostWire::deserialize(deserializer)? {
            TriggerPreviewCostWire::ExactZero {
                currency,
                amount_micros,
                display,
            } => {
                if currency != "USD" {
                    return Err(de::Error::custom("exact-zero cost currency must be USD"));
                }
                if amount_micros != 0 {
                    return Err(de::Error::custom("exact-zero cost amount must be zero"));
                }
                Ok(Self::ExactZero {
                    currency,
                    amount_micros,
                    display,
                })
            }
            TriggerPreviewCostWire::ContributorEstimate {
                currency,
                amount_micros,
                display,
                quoted_at,
            } => {
                if currency != "USD" {
                    return Err(de::Error::custom("contributor cost currency must be USD"));
                }
                Ok(Self::ContributorEstimate {
                    currency,
                    amount_micros,
                    display,
                    quoted_at,
                })
            }
            TriggerPreviewCostWire::QuoteUnavailable => Ok(Self::QuoteUnavailable),
        }
    }
}

/// Side-effect-free public README Trigger preview.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "TriggerPreviewWire")]
pub struct TriggerPreview {
    pub public_id: String,
    pub availability: TriggerPreviewAvailability,
    pub unavailable_reason: Option<TriggerPreviewUnavailableReason>,
    pub publisher_display_identity: String,
    pub repository_id: u64,
    pub repository_owner: String,
    pub repository_name: String,
    pub visibility: RepositoryVisibility,
    pub allow_forking: bool,
    pub public_summary: String,
    pub permission_ceiling: TriggerPermissionMode,
    pub required_capabilities: Vec<TriggerRequiredCapability>,
    pub resource_default: TriggerSessionSize,
    pub resource_ceiling: TriggerSessionSize,
    pub timeout_default_seconds: u32,
    pub timeout_ceiling_seconds: u32,
    pub billing_mode: TriggerBillingMode,
    pub pricing_policy_reference: String,
    pub estimated_cost: TriggerPreviewCost,
    pub revision_digest: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TriggerPreviewWire {
    public_id: String,
    availability: TriggerPreviewAvailability,
    unavailable_reason: Option<TriggerPreviewUnavailableReason>,
    publisher_display_identity: String,
    repository_id: u64,
    repository_owner: String,
    repository_name: String,
    visibility: RepositoryVisibility,
    allow_forking: bool,
    public_summary: String,
    permission_ceiling: TriggerPermissionMode,
    required_capabilities: Vec<TriggerRequiredCapability>,
    resource_default: TriggerSessionSize,
    resource_ceiling: TriggerSessionSize,
    timeout_default_seconds: u32,
    timeout_ceiling_seconds: u32,
    billing_mode: TriggerBillingMode,
    pricing_policy_reference: String,
    estimated_cost: TriggerPreviewCost,
    revision_digest: String,
}

impl TryFrom<TriggerPreviewWire> for TriggerPreview {
    type Error = &'static str;

    fn try_from(value: TriggerPreviewWire) -> std::result::Result<Self, Self::Error> {
        match (value.availability, value.unavailable_reason) {
            (TriggerPreviewAvailability::Unavailable, Some(_))
            | (TriggerPreviewAvailability::Published | TriggerPreviewAvailability::Retired, None) =>
                {}
            _ => return Err("Trigger preview availability and unavailable_reason disagree"),
        }
        Ok(Self {
            public_id: value.public_id,
            availability: value.availability,
            unavailable_reason: value.unavailable_reason,
            publisher_display_identity: value.publisher_display_identity,
            repository_id: value.repository_id,
            repository_owner: value.repository_owner,
            repository_name: value.repository_name,
            visibility: value.visibility,
            allow_forking: value.allow_forking,
            public_summary: value.public_summary,
            permission_ceiling: value.permission_ceiling,
            required_capabilities: value.required_capabilities,
            resource_default: value.resource_default,
            resource_ceiling: value.resource_ceiling,
            timeout_default_seconds: value.timeout_default_seconds,
            timeout_ceiling_seconds: value.timeout_ceiling_seconds,
            billing_mode: value.billing_mode,
            pricing_policy_reference: value.pricing_policy_reference,
            estimated_cost: value.estimated_cost,
            revision_digest: value.revision_digest,
        })
    }
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
    pub selected_service_id: String,
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
    pub role: TriggerPublisherRepositoryRole,
    pub verified_at: String,
}

/// Publisher role proven against the upstream repository.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerPublisherRepositoryRole {
    Owner,
    Admin,
    Maintain,
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
    pub state: TriggerConnectionState,
    pub account_id: Option<u64>,
    pub account_node_id: Option<String>,
    pub account_login: Option<String>,
    pub avatar_url: Option<String>,
    pub granted_scopes: Vec<String>,
    pub consent_observed_at: Option<String>,
}

/// Reusable NyxID connection state.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerConnectionState {
    Active,
    Inactive,
    Disconnected,
    Revoked,
    Expired,
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

/// Why an otherwise active Launch is waiting for an explicit recovery action.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchBlockedReason {
    FeaturePaused,
    OwnerCredentialUnavailable,
    ServiceInactive,
    SessionRotating,
    SessionRecovering,
    RecoveryRequired,
    RevocationPending,
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

/// Stable Launch progress step key.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchStepKey {
    Review,
    Authentication,
    Preflight,
    Fork,
    Branch,
    Workspace,
    Sandbox,
    Session,
    Reach,
    InitialPrompt,
    Checkpoint,
    Diff,
    PublishBranch,
    PullRequest,
    Cleanup,
}

/// One stable progress row shown to the contributor.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchStep {
    pub key: LaunchStepKey,
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

/// Public Launch failure class.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchFailureClass {
    AuthRequired,
    OwnerCredentialUnavailable,
    ServiceInactive,
    RetryableTransient,
    AmbiguousExternal,
    ConflictStaleHead,
    ResourceQuota,
    PolicyUserAction,
    RecoveryRequired,
    TerminalValidation,
    TerminalSecurity,
    FeaturePaused,
}

/// Public-safe Launch failure.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchFailure {
    pub class: LaunchFailureClass,
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
    pub work_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_size: Option<TriggerSessionSize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u32>,
    #[serde(default)]
    pub optional_checks: [String; 0],
}

/// Exact effective configuration sealed for Start.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveConfiguration {
    pub rendered_prompt: String,
    pub work_label: String,
    pub permission: TriggerPermissionMode,
    pub resource_size: TriggerSessionSize,
    pub timeout_seconds: u32,
    pub optional_checks: [String; 0],
    pub digest: String,
}

/// Exact deployment rate terms included in a contributor quote.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerRate {
    pub cpu_micros_per_cpu_hour: u64,
    pub memory_micros_per_gib_hour: u64,
}

/// Server-owned price or explicit zero-cost promise sealed into a Launch review.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchPricingSnapshot {
    pub policy_reference: String,
    pub billing_mode: TriggerBillingMode,
    pub rate_card: Option<String>,
    pub rate: Option<TriggerRate>,
    pub currency: String,
    pub cpu_millis: u32,
    pub memory_mib: u32,
    pub micros_per_hour: u64,
    pub estimated_micros: u64,
    pub quoted_at: String,
    pub expires_at: String,
}

/// Closed kernel identity admitted for a Trigger-owned session.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum TriggerKernel {
    /// Historical route retained for reads, cancellation, session access, and
    /// immutable artifact download; it cannot authorize new Trigger work.
    #[serde(rename = "codex")]
    Codex,
    /// Current dedicated Trigger runtime route.
    #[serde(rename = "codex-trigger-native-v1")]
    CodexTriggerNativeV1,
}

impl TriggerKernel {
    /// Whether this is the current route that may admit new Trigger work.
    pub const fn is_current(self) -> bool {
        matches!(self, Self::CodexTriggerNativeV1)
    }

    /// Whether this is the historical route exposed only through the bounded
    /// read, cancellation, session, and retained-artifact surface.
    pub const fn is_legacy_read_only(self) -> bool {
        matches!(self, Self::Codex)
    }
}

/// Locked kernel, pinned runtime identity, and demonstrated capabilities shown during review.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchRuntimeAdmission {
    pub kernel: TriggerKernel,
    pub runtime_version: String,
    pub supported_capabilities: Vec<TriggerRequiredCapability>,
    pub observed_at: String,
}

/// Review- or Start-time observation of owner and deployment capacity.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchQuotaSnapshot {
    pub live_sessions: u32,
    pub live_session_limit: Option<u32>,
    pub reserved_cpu_millis: u64,
    pub reserved_memory_mib: u64,
    pub free_cpu_millis: Option<u64>,
    pub free_memory_mib: Option<u64>,
    pub checked_at: String,
}

/// Fresh server admission recorded immediately before Start is accepted.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchStartAdmission {
    pub runtime_version: String,
    pub pricing_policy_reference: String,
    pub rate_card: Option<String>,
    pub quota: LaunchQuotaSnapshot,
    pub checked_at: String,
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
#[serde(try_from = "DiffFileWire")]
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
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    pub patch_complete: bool,
}

#[derive(Deserialize)]
#[serde(transparent)]
struct RequiredNullableLineCount(Option<u64>);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiffFileWire {
    path: String,
    change: DiffChangeKind,
    binary: bool,
    old_mode: Option<u32>,
    new_mode: Option<u32>,
    old_sha256: Option<String>,
    new_sha256: Option<String>,
    #[serde(default)]
    old_provider_object_id: Option<String>,
    #[serde(default)]
    new_provider_object_id: Option<String>,
    old_bytes: u64,
    new_bytes: u64,
    additions: RequiredNullableLineCount,
    deletions: RequiredNullableLineCount,
    #[serde(default)]
    patch: Option<String>,
    patch_complete: bool,
}

impl TryFrom<DiffFileWire> for DiffFile {
    type Error = &'static str;

    fn try_from(value: DiffFileWire) -> std::result::Result<Self, Self::Error> {
        let additions = value.additions.0;
        let deletions = value.deletions.0;
        match (value.binary, additions, deletions) {
            (false, Some(_), Some(_)) | (true, None, None) => {}
            _ => return Err("Diff file line statistics disagree with its binary state"),
        }
        Ok(Self {
            path: value.path,
            change: value.change,
            binary: value.binary,
            old_mode: value.old_mode,
            new_mode: value.new_mode,
            old_sha256: value.old_sha256,
            new_sha256: value.new_sha256,
            old_provider_object_id: value.old_provider_object_id,
            new_provider_object_id: value.new_provider_object_id,
            old_bytes: value.old_bytes,
            new_bytes: value.new_bytes,
            additions,
            deletions,
            patch: value.patch,
            patch_complete: value.patch_complete,
        })
    }
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
    pub validation: DiffValidationOutcome,
    pub checks: Vec<DiffCheck>,
    pub created_at: String,
    pub delete_after: String,
    pub download_url: String,
}

/// Public outcome of validating a sealed diff artifact.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffValidationOutcome {
    Valid,
    NoChanges,
    StaleHead,
    WorkflowScopeRequired,
    TooManyChanges,
    TooManyBytes,
    UnsupportedFile,
    MissingBaseline,
    CorruptCheckpoint,
    ArtifactTooLarge,
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
    pub state: PublicationState,
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

/// Guarded branch-publication state.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationState {
    IntentRecorded,
    ObjectsWritten,
    RefUpdatePending,
    Published,
    Conflict,
    RecoveryRequired,
    Failed,
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

/// Aggregate provider check state for a pull request.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestCheckSummaryState {
    Pending,
    Passing,
    Failing,
    AwaitingMaintainerApproval,
    Unavailable,
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
    pub checks: PullRequestCheckSummaryState,
    pub confirmation_snapshot_digest: String,
    pub confirmed_at: String,
    pub created_at: String,
    pub updated_at: String,
    pub provider_created_at: Option<String>,
    pub provider_updated_at: Option<String>,
    /// Safe failure from the latest read-only provider status observation.
    pub status_failure: Option<LaunchFailure>,
    /// Provider correlation identifier for the latest status observation.
    pub provider_request_id: Option<String>,
    /// Whether the observed provider head still equals CMA's retained Launch head.
    pub head_matches_launch: Option<bool>,
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
    pub state: CapabilityRevocationState,
    pub policy_generation: u64,
    pub policy_fence: u64,
    pub runtime_audience: String,
    pub expires_at_epoch_seconds: u64,
    pub numeric_binding_observed_at: String,
    pub refreshed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revocation_requested_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revocation_effective_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_refusal: Option<CapabilityRefusal>,
    pub tombstone_acknowledged: bool,
    pub agent_key_revocation_confirmed: bool,
}

/// Positive durable evidence that CMA retired credentials bound to a Launch session.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchSessionCredentialRetirement {
    pub session_id: String,
    pub settled_at: String,
}

/// Restricted service-gateway capability lifecycle state.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityRevocationState {
    Active,
    RevocationPending,
    Revoked,
    Expired,
}

/// Closed public refusal reason from the restricted service gateway.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityRefusal {
    MissingPolicy,
    UnknownVersion,
    Expired,
    StaleGeneration,
    Revoked,
    MalformedRequest,
    ResourceMismatch,
    OperationDenied,
    AudienceMismatch,
    SignatureInvalid,
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

/// Durable external-effect class represented by a Launch event.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchEffectKind {
    Preflight,
    Fork,
    Branch,
    WorkspaceImport,
    Sandbox,
    Session,
    Reach,
    InitialDispatch,
    Checkpoint,
    Diff,
    BranchPublication,
    PullRequest,
    Cleanup,
}

/// Explicit contributor confirmation recorded in Launch history.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchConfirmationAction {
    Start,
    WorkflowRisk,
    ReviewDiff,
    PublishBranch,
    OpenPullRequest,
    UpdatePullRequest,
    RecoverPrompt,
    NewLaunch,
    Cancel,
    Complete,
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
    #[serde(default)]
    pub recovery_generation: u32,
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
    pub blocked_reason: Option<LaunchBlockedReason>,
    pub failure: Option<LaunchFailure>,
    pub resume_state: Option<LaunchState>,
    pub overrides: LaunchOverrides,
    pub effective: EffectiveConfiguration,
    pub pricing: Option<LaunchPricingSnapshot>,
    pub runtime_admission: Option<LaunchRuntimeAdmission>,
    pub review_quota: Option<LaunchQuotaSnapshot>,
    pub start_admission: Option<LaunchStartAdmission>,
    pub connection: Option<TriggerConnection>,
    pub workflow_paths: Vec<String>,
    pub workflow_risk_acknowledged: bool,
    pub steps: Vec<LaunchStep>,
    pub fork: Option<ForkRecord>,
    pub branch: Option<TriggerBranchRecord>,
    pub workspace: Option<JsonObject>,
    pub session: Option<JsonObject>,
    pub session_credential_retirement: Option<LaunchSessionCredentialRetirement>,
    pub reach: Option<LaunchReachStatus>,
    pub delivery: Option<PromptDelivery>,
    pub contribution_checkpoint_version: Option<u64>,
    pub diff: Option<DiffArtifact>,
    pub contribution: Option<ContributionRecord>,
    pub pull_request: Option<PullRequestRecord>,
    pub created_at: String,
    pub updated_at: String,
    pub draft_expires_at: String,
    pub started_at: Option<String>,
    pub terminal_at: Option<String>,
}

/// Closed public Launch event taxonomy.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LaunchEventKind {
    Created {
        steps: Vec<LaunchStep>,
    },
    DraftUpdated,
    ConnectionSelected,
    Started,
    StateChanged {
        from: LaunchState,
        to: LaunchState,
    },
    StepChanged {
        step: LaunchStep,
    },
    Blocked {
        reason: LaunchBlockedReason,
    },
    Resumed {
        reason: LaunchBlockedReason,
    },
    Failure {
        failure: LaunchFailure,
    },
    EffectReconciled {
        effect: LaunchEffectKind,
    },
    RecoveryAuthorized {
        effect: LaunchEffectKind,
    },
    DeliveryChanged {
        state: PromptDeliveryState,
    },
    ReachChanged {
        reach: LaunchReachStatus,
    },
    PullRequestStatusChanged {
        state: PullRequestState,
        mergeability: Mergeability,
        checks: PullRequestCheckSummaryState,
        failure: Option<LaunchFailure>,
    },
    PullRequestStatusRefreshRequested,
    ConfirmationRecorded {
        action: LaunchConfirmationAction,
    },
    CleanupPlanned {
        target_count: usize,
    },
    CleanupCompleted {
        target_count: usize,
    },
    Cancelled,
}

/// Authoritative compact state captured with each replayable event.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchProgressState {
    pub state: LaunchState,
    pub blocked_reason: Option<LaunchBlockedReason>,
    pub failure: Option<LaunchFailure>,
    pub resume_state: Option<LaunchState>,
    pub reach: Option<LaunchReachStatus>,
    pub delivery: Option<PromptDelivery>,
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
    pub progress: LaunchProgressState,
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
    pub session_id: Option<String>,
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

/// Explicit authorization for one replacement initial-prompt dispatch.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoverInitialPromptRequest {
    pub expected_revision: u64,
    pub snapshot_digest: String,
    pub acknowledge_duplicate_execution_risk: bool,
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

/// Optimistic command that queues one immediate read-only pull-request status observation.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PullRequestStatusRefreshRequest {
    pub expected_revision: u64,
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

    /// Revoke one exact reusable NyxID service without deleting Launch history.
    pub async fn revoke_trigger_connection(
        &self,
        service_id: &str,
        idempotency_key: &str,
    ) -> Result<TriggerConnection> {
        identifier(service_id, "service id")?;
        let connection: TriggerConnection = self
            .command(
                Method::DELETE,
                &["trigger-connections", service_id],
                HashMap::<String, Value>::new(),
                StatusCode::OK,
                idempotency_key,
            )
            .await?;
        if connection.service_id != service_id {
            return Err(Error::protocol(
                "CMA returned revocation state for a different GitHub connection",
            ));
        }
        Ok(connection)
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
        if let Some(session_id) = page.session_id {
            identifier(&session_id, "session id")?;
            query.push(("session_id", session_id));
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

    /// Authorize one replacement initial-prompt dispatch after an ambiguous write.
    pub async fn recover_initial_prompt(
        &self,
        launch_id: &str,
        request: RecoverInitialPromptRequest,
        idempotency_key: &str,
    ) -> Result<Launch> {
        identifier(launch_id, "Launch id")?;
        self.command(
            Method::POST,
            &["launches", launch_id, "recover-initial-prompt"],
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

    /// Queue an immediate read-only provider status observation for an open pull request.
    pub async fn refresh_pull_request_status(
        &self,
        launch_id: &str,
        request: PullRequestStatusRefreshRequest,
        idempotency_key: &str,
    ) -> Result<Launch> {
        identifier(launch_id, "Launch id")?;
        self.command(
            Method::POST,
            &["launches", launch_id, "pull-request", "refresh"],
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

    /// Finish CMA orchestration after handing an open pull request to its maintainers.
    pub async fn complete_launch(
        &self,
        launch_id: &str,
        request: LaunchConfirmationRequest,
        idempotency_key: &str,
    ) -> Result<Launch> {
        identifier(launch_id, "Launch id")?;
        self.command(
            Method::POST,
            &["launches", launch_id, "complete"],
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
        if limit == 0 || limit > 500 {
            return Err(Error::protocol(
                "Launch progress limit must be from 1 through 500",
            ));
        }
        let progress: LaunchProgress = self
            .request_public_json(
                Method::GET,
                &["launches", launch_id, "events"],
                &[("after", after.to_string()), ("limit", limit.to_string())],
                None,
                StatusCode::OK,
                false,
            )
            .await?;
        normalize_launch_progress(launch_id, after, progress)
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

fn normalize_launch_progress(
    requested_launch_id: &str,
    after: u64,
    mut progress: LaunchProgress,
) -> Result<LaunchProgress> {
    if progress.launch_id != requested_launch_id {
        return Err(Error::protocol(format!(
            "Launch progress response belongs to {}, not {requested_launch_id}",
            progress.launch_id
        )));
    }

    let mut accepted: Vec<LaunchEvent> = Vec::with_capacity(progress.events.len());
    let mut expected_sequence = after.checked_add(1);
    for event in progress.events {
        if event.launch_id != progress.launch_id {
            return Err(Error::protocol(format!(
                "Launch progress event {} belongs to {}, not {}",
                event.sequence, event.launch_id, progress.launch_id
            )));
        }
        if let Some(previous) = accepted.last()
            && event.sequence == previous.sequence
        {
            if event == *previous {
                continue;
            }
            return Err(Error::protocol(format!(
                "Launch progress sequence {} has conflicting duplicate envelopes",
                event.sequence
            )));
        }

        let Some(expected) = expected_sequence else {
            return Err(Error::protocol(
                "Launch progress returned an event after the maximum sequence",
            ));
        };
        if event.sequence != expected {
            let problem = if event.sequence > expected {
                "contains a sequence gap"
            } else {
                "contains an out-of-order or unprovable duplicate event"
            };
            return Err(Error::protocol(format!(
                "Launch progress {problem}: expected {expected}, received {}",
                event.sequence
            )));
        }
        expected_sequence = event.sequence.checked_add(1);
        accepted.push(event);
    }

    let expected_cursor = accepted.last().map_or(after, |event| event.sequence);
    if progress.next_cursor != expected_cursor {
        return Err(Error::protocol(format!(
            "Launch progress next_cursor {} does not match accepted sequence {expected_cursor}",
            progress.next_cursor
        )));
    }
    progress.events = accepted;
    Ok(progress)
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
