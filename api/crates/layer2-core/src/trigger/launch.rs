use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{
    AuthorityProvenance, CheckpointId, ConnectionSnapshot, DiffArtifact, DiffArtifactId,
    EditableField, LaunchId, PullRequestState, RepositoryIdentity, RequiredCapability,
    TriggerRevision, TriggerValidationError, WorkspaceCheckpoint, WorkspaceCheckpointPurpose,
    WorkspaceManifest,
};
use crate::{
    CapabilityRevocationState, PermissionMode, Rate, RateCardId, SessionId, SessionSize, SubjectId,
};

mod admission;
mod capability;
mod checkpoint;
mod effect;
mod pull_request;
mod replay;

pub use admission::*;
pub use pull_request::PULL_REQUEST_STATUS_INTERVAL_SECONDS;
pub use replay::{
    replay_launch_progress, LaunchProgressProjection, LaunchProgressState, LaunchReplayError,
};

pub const LAUNCH_SCHEMA_VERSION: u32 = 1;
/// Recent events retained beside the aggregate for worker diagnostics and the
/// bounded support timeline. The canonical stream belongs to `LaunchStore`.
pub const LAUNCH_EVENT_TAIL_LIMIT: usize = 512;
pub const MAX_LAUNCH_EFFECT_ATTEMPTS: u32 = 5;
pub const MAX_LAUNCH_RECONCILIATION_ATTEMPTS: u32 = 20;
/// A saved review remains startable for one day unless a stricter quote expires first.
pub const LAUNCH_DRAFT_TTL_SECONDS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::FailedTerminal | Self::Cancelled
        )
    }

    pub fn is_active(self) -> bool {
        !self.is_terminal()
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchFailure {
    pub class: FailureClass,
    pub code: String,
    pub detail: String,
    pub remediation: String,
    pub correlation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<String>,
    pub observed_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchStep {
    pub key: LaunchStepKey,
    pub sequence: u32,
    pub status: LaunchStepStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    pub attempt: u32,
    pub safe_detail: String,
    pub remediation: String,
    #[serde(default)]
    pub external_resource_ids: BTreeMap<String, String>,
    pub worker_fencing_token: u64,
    pub correlation_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
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
        reach: super::ReachStatus,
    },
    PullRequestStatusChanged {
        state: super::PullRequestState,
        mergeability: super::Mergeability,
        checks: super::CheckSummaryState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<LaunchFailure>,
    },
    PullRequestStatusRefreshRequested,
    ConfirmationRecorded {
        action: ConfirmationAction,
    },
    CleanupPlanned {
        target_count: usize,
    },
    CleanupCompleted {
        target_count: usize,
    },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LaunchEvent {
    pub sequence: u64,
    pub launch_id: LaunchId,
    pub occurred_at: String,
    pub correlation_id: String,
    pub progress: LaunchProgressState,
    pub kind: LaunchEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerLease {
    pub holder: String,
    pub fencing_token: u64,
    pub acquired_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectState {
    Queued,
    Claimed,
    Reconciling,
    Succeeded,
    FailedRetryable,
    FailedTerminal,
    RecoveryRequired,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectRecord {
    pub kind: LaunchEffectKind,
    pub state: EffectState,
    pub intent_key: String,
    pub attempt: u32,
    #[serde(default)]
    pub reconciliation_attempt: u32,
    pub fence: u64,
    /// Exact credential-free authority evidence committed before this attempt
    /// was allowed to cross an irreversible GitHub boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority_admission: Option<AuthorityAdmissionSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_request_id: Option<String>,
    #[serde(default)]
    pub external_resource_ids: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_attempt_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<LaunchFailure>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectClaimMode {
    Execute,
    Reconcile,
}

/// Durable authority to execute or reconcile one logical external effect.
///
/// This value contains no credential. Its fence becomes stale as soon as a
/// replacement worker acquires the Launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectClaim {
    pub kind: LaunchEffectKind,
    pub mode: EffectClaimMode,
    pub intent_key: String,
    pub attempt: u32,
    pub reconciliation_attempt: u32,
    pub fence: u64,
}

/// One successful, fenced effect completion.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectCompletion {
    pub result: LaunchEffectResult,
    pub safe_detail: String,
    pub external_resource_ids: BTreeMap<String, String>,
    pub now: String,
    pub correlation_id: String,
}

/// Exact authority and repository state installed by one capability renewal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityRenewalRecord {
    pub reach: super::ReachStatus,
    pub connection: ConnectionSnapshot,
    pub authority: AuthorityProvenance,
    pub branch: BranchRecord,
}

/// Typed durable result for one effect. Provider adapters cannot advance a
/// Launch by returning an unstructured success flag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)] // Durable effect values retain their exact public shape.
pub enum LaunchEffectResult {
    Preflight {
        connection: ConnectionSnapshot,
        authority: AuthorityProvenance,
        source_sha: String,
        #[serde(default)]
        workflow_paths: Vec<String>,
    },
    Fork {
        fork: ForkRecord,
    },
    Branch {
        branch: BranchRecord,
    },
    WorkspaceImport {
        workspace: WorkspaceRecord,
    },
    Sandbox,
    Session {
        session: SessionLink,
    },
    Reach {
        reach: super::ReachStatus,
        connection: ConnectionSnapshot,
        authority: AuthorityProvenance,
        branch: BranchRecord,
    },
    InitialDispatch {
        delivery: PromptDelivery,
    },
    Checkpoint {
        checkpoint: WorkspaceCheckpoint,
    },
    Diff {
        diff: DiffArtifact,
    },
    BranchPublication {
        contribution: super::ContributionRecord,
    },
    PullRequest {
        pull_request: super::PullRequestRecord,
    },
    Cleanup {
        receipt: CleanupReceipt,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct LaunchOverrides {
    #[serde(default)]
    pub task_inputs: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_size: Option<SessionSize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u32>,
    #[serde(default)]
    pub optional_checks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectiveConfiguration {
    pub rendered_prompt: String,
    pub work_label: String,
    pub permission: PermissionMode,
    pub resource_size: SessionSize,
    pub timeout_seconds: u32,
    #[serde(default)]
    pub optional_checks: Vec<String>,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PricingSnapshot {
    pub policy_reference: String,
    pub billing_mode: super::BillingMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_card: Option<RateCardId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<Rate>,
    pub currency: String,
    pub cpu_millis: u32,
    pub memory_mib: u32,
    pub micros_per_hour: u64,
    pub estimated_micros: u64,
    pub quoted_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeAdmissionSnapshot {
    /// Opaque runtime choice locked during review. The composition root owns
    /// which concrete identifiers are eligible for this product workflow.
    pub kernel: crate::KernelId,
    pub runtime_version: String,
    #[serde(default)]
    pub supported_capabilities: Vec<RequiredCapability>,
    pub observed_at: String,
}

/// Runtime route carried by one complete durable admission snapshot.
///
/// `Invalid` includes missing snapshots, unknown kernels, and any partial or
/// contradictory version, timestamp, or capability inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeAdmissionRoute {
    Current,
    Historical,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaSnapshot {
    pub live_sessions: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_session_limit: Option<u32>,
    pub reserved_cpu_millis: u64,
    pub reserved_memory_mib: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_cpu_millis: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_memory_mib: Option<u64>,
    pub checked_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartAdmissionSnapshot {
    pub runtime_version: String,
    pub pricing_policy_reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_card: Option<RateCardId>,
    pub quota: QuotaSnapshot,
    pub checked_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchReviewAdmission {
    pub pricing: PricingSnapshot,
    pub runtime: RuntimeAdmissionSnapshot,
    pub quota: QuotaSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationAction {
    Start,
    WorkflowRisk,
    ReviewDiff,
    PublishBranch,
    OpenPullRequest,
    UpdatePullRequest,
    Complete,
    RecoverPrompt,
    NewLaunch,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Confirmation {
    pub action: ConfirmationAction,
    pub subject: SubjectId,
    pub snapshot_digest: String,
    pub confirmed_at: String,
    pub idempotency_key_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkRecord {
    pub repository: RepositoryIdentity,
    pub reused: bool,
    pub source_revision: String,
    pub canonical_url: String,
    pub ready_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchRecord {
    pub name: String,
    pub expected_head: String,
    pub current_head: String,
    pub canonical_url: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRecord {
    pub baseline: WorkspaceManifest,
    /// Machine-owned object containing the exact baseline bytes.
    ///
    /// The reference is useful only through the owner- and Launch-scoped
    /// object-store port. It is not a bearer credential.
    pub baseline_object_reference: String,
    pub baseline_archive_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_checkpoint: Option<WorkspaceCheckpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_checkpoint: Option<WorkspaceCheckpoint>,
    /// Obsolete CMA-owned object awaiting idempotent post-commit deletion.
    /// It remains durable until object deletion and a second aggregate CAS
    /// both succeed, so a process crash cannot turn retention debt into a leak.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_checkpoint: Option<WorkspaceCheckpoint>,
    /// Exact checkpoint sealed by the user's latest Review Diff command.
    /// Lifecycle captures may advance recovery without changing this pin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed_contribution_checkpoint: Option<WorkspaceCheckpoint>,
    /// The one version currently allowed to cross the runtime/object boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_intent: Option<super::WorkspaceCheckpointIntent>,
    /// Background durability state, deliberately separate from the explicit
    /// contribution progress step.
    #[serde(default)]
    pub lifecycle_checkpoint: super::LifecycleCheckpointStatus,
    /// Monotonic capture high-water, including coherent captures whose tree was
    /// unchanged and therefore did not create another durable object.
    #[serde(default)]
    pub checkpoint_version: u64,
    pub workspace_epoch: u64,
    pub retention_policy_id: String,
    pub delete_after: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLink {
    pub session_id: SessionId,
    pub environment_profile_id: String,
    pub runtime_generation: u64,
    pub created_at: String,
}

/// Positive durable evidence that CMA credential custody retired every key
/// assigned to this Launch's deterministic session identity.
///
/// This is intentionally separate from Reach: a key can be installed before a
/// runtime policy is admitted, and settling it must not fabricate policy or
/// tombstone evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCredentialRetirement {
    pub session_id: SessionId,
    pub settled_at: String,
}

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptDelivery {
    pub launch_id: LaunchId,
    pub session_id: SessionId,
    pub revision_digest: String,
    pub origin_key: String,
    pub payload_hash: String,
    pub state: PromptDeliveryState,
    pub attempt: u32,
    /// Number of explicitly authorized replacement-dispatch generations.
    ///
    /// Proven pre-write retries stay within one generation. An ambiguous write
    /// consumes the generation and another send remains fenced until a new
    /// `RecoverPrompt` confirmation advances this value.
    #[serde(default)]
    pub recovery_generation: u32,
    pub dispatch_fence_sequence: Option<u64>,
    pub queued_at: String,
    pub updated_at: String,
}

/// A machine-owned object class that retention cleanup is allowed to delete.
///
/// The closed vocabulary is the safety property. It contains no sandbox and
/// no GitHub fork, branch, commit, pull request, or other provider resource.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CleanupTarget {
    Baseline,
    Checkpoint { checkpoint_id: CheckpointId },
    DiffArtifact { artifact_id: DiffArtifactId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupPlan {
    pub retention_policy_id: String,
    pub delete_after: String,
    pub targets: Vec<CleanupTarget>,
    pub planned_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupReceipt {
    pub targets: Vec<CleanupTarget>,
    pub completed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupRecord {
    pub plan: CleanupPlan,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Launch {
    pub schema_version: u32,
    pub id: LaunchId,
    pub owner: SubjectId,
    pub revision: TriggerRevision,
    pub revision_digest: String,
    /// Exact upstream commit used by every fork, workspace, and contribution
    /// effect. Branch-head mode refreshes this at Start.
    #[serde(default)]
    pub source_sha: String,
    pub state: LaunchState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<LaunchBlockedReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<LaunchFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_state: Option<LaunchState>,
    pub overrides: LaunchOverrides,
    pub effective: EffectiveConfiguration,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing: Option<PricingSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_admission: Option<RuntimeAdmissionSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_quota: Option<QuotaSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_admission: Option<StartAdmissionSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<ConnectionSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<AuthorityProvenance>,
    #[serde(default)]
    pub workflow_paths: Vec<String>,
    pub workflow_risk_acknowledged: bool,
    #[serde(default)]
    pub steps: Vec<LaunchStep>,
    #[serde(default)]
    pub effects: Vec<EffectRecord>,
    /// Monotonic high-water for the independent Launch event stream. This is
    /// separate from the bounded recent `events` tail and survives compaction.
    #[serde(default)]
    pub event_sequence: u64,
    #[serde(default)]
    pub events: Vec<LaunchEvent>,
    #[serde(default)]
    pub confirmations: Vec<Confirmation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork: Option<ForkRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<BranchRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_credential_retirement: Option<SessionCredentialRetirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach: Option<super::ReachStatus>,
    /// A policy generation admitted under the current capability-worker fence.
    /// Its presence means installation may have been attempted and takeover
    /// must not resend it blindly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_authority_admission: Option<CapabilityAuthorityAdmission>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<PromptDelivery>,
    /// Checkpoint version selected by the latest explicit diff-review request.
    /// This prevents a later contribution from accidentally reviewing an old
    /// complete checkpoint merely because one is already retained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contribution_checkpoint_version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<DiffArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contribution: Option<super::ContributionRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<super::PullRequestRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<CleanupRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<WorkerLease>,
    pub fencing_counter: u64,
    pub revision_number: u64,
    pub created_at: String,
    pub updated_at: String,
    /// Server-derived deadline for every pre-Start mutation.
    #[serde(default)]
    pub draft_expires_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_at: Option<String>,
}

impl Launch {
    pub fn new(
        owner: SubjectId,
        revision: TriggerRevision,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<Self, TriggerValidationError> {
        revision.content.validate()?;
        let now = now.into();
        let draft_expires_at = draft_expiry(&now)?;
        let effective = effective_configuration(&revision, &LaunchOverrides::default())?;
        let id = LaunchId::new();
        let correlation_id = correlation_id.into();
        let steps = default_steps(&correlation_id);
        let mut launch = Self {
            schema_version: LAUNCH_SCHEMA_VERSION,
            id: id.clone(),
            owner,
            revision_digest: revision.digest.clone(),
            source_sha: revision
                .content
                .upstream
                .observed_source_sha
                .clone()
                .unwrap_or_default(),
            revision,
            state: LaunchState::Draft,
            blocked_reason: None,
            failure: None,
            resume_state: None,
            overrides: LaunchOverrides::default(),
            effective,
            pricing: None,
            runtime_admission: None,
            review_quota: None,
            start_admission: None,
            connection: None,
            authority: None,
            workflow_paths: Vec::new(),
            workflow_risk_acknowledged: false,
            steps,
            effects: Vec::new(),
            event_sequence: 0,
            events: Vec::new(),
            confirmations: Vec::new(),
            fork: None,
            branch: None,
            workspace: None,
            session: None,
            session_credential_retirement: None,
            reach: None,
            capability_authority_admission: None,
            delivery: None,
            contribution_checkpoint_version: None,
            diff: None,
            contribution: None,
            pull_request: None,
            cleanup: None,
            lease: None,
            fencing_counter: 0,
            revision_number: 0,
            created_at: now.clone(),
            updated_at: now.clone(),
            draft_expires_at,
            started_at: None,
            terminal_at: None,
        };
        let initial_steps = launch.steps.clone();
        launch.push_event(
            now,
            correlation_id,
            LaunchEventKind::Created {
                steps: initial_steps,
            },
        )?;
        Ok(launch)
    }

    /// Classify a retained runtime admission against the current
    /// composition-selected route and capability inventory.
    ///
    /// A durable snapshot is authority: absent or partial fields are invalid
    /// and must never be inferred from today's configuration.
    pub fn runtime_admission_route(
        &self,
        current_kernel: &str,
        historical_kernel: &str,
        expected_capabilities: &[RequiredCapability],
    ) -> RuntimeAdmissionRoute {
        self.validated_runtime_admission_route(
            current_kernel,
            historical_kernel,
            expected_capabilities,
        )
        .unwrap_or(RuntimeAdmissionRoute::Invalid)
    }

    /// Return a readable current or exact historical route while preserving the
    /// precise malformed field for a quarantined snapshot.
    pub fn validated_runtime_admission_route(
        &self,
        current_kernel: &str,
        historical_kernel: &str,
        expected_capabilities: &[RequiredCapability],
    ) -> Result<RuntimeAdmissionRoute, TriggerValidationError> {
        let runtime = self.runtime_admission.as_ref().ok_or_else(|| {
            TriggerValidationError::field(
                "runtime_admission",
                "the durable runtime admission snapshot is missing",
            )
        })?;
        if current_kernel.trim().is_empty()
            || historical_kernel.trim().is_empty()
            || current_kernel == historical_kernel
        {
            return Err(TriggerValidationError::field(
                "runtime_admission.kernel",
                "has no composition-selected current runtime route",
            ));
        }
        if runtime.runtime_version.trim().is_empty() {
            return Err(TriggerValidationError::field(
                "runtime_admission.runtime_version",
                "does not pin a runtime version",
            ));
        }
        if crate::seconds_from_rfc3339(&runtime.observed_at).is_none() {
            return Err(TriggerValidationError::field(
                "runtime_admission.observed_at",
                "does not contain a CMA timestamp",
            ));
        }

        let expected: BTreeSet<_> = expected_capabilities.iter().copied().collect();
        let published: BTreeSet<_> = self
            .revision
            .content
            .session
            .required_capabilities
            .iter()
            .copied()
            .collect();
        let supported: BTreeSet<_> = runtime.supported_capabilities.iter().copied().collect();
        if expected.len() != expected_capabilities.len()
            || published.len() != self.revision.content.session.required_capabilities.len()
            || supported.len() != runtime.supported_capabilities.len()
            || published != expected
            || supported != expected
        {
            return Err(TriggerValidationError::field(
                "runtime_admission.supported_capabilities",
                "does not exactly capture the selected capability inventory",
            ));
        }
        match runtime.kernel.as_str() {
            kernel if kernel == current_kernel => Ok(RuntimeAdmissionRoute::Current),
            kernel if kernel == historical_kernel => Ok(RuntimeAdmissionRoute::Historical),
            _ => Err(TriggerValidationError::field(
                "runtime_admission.kernel",
                "does not name a retained Trigger runtime route",
            )),
        }
    }

    /// Validate authority for new work on the current runtime route. Exact
    /// historical admissions remain readable through
    /// `validated_runtime_admission_route` but can never satisfy this
    /// current-only admission boundary.
    pub fn validate_runtime_admission_route(
        &self,
        expected_kernel: &str,
        historical_kernel: &str,
        expected_capabilities: &[RequiredCapability],
    ) -> Result<(), TriggerValidationError> {
        match self.validated_runtime_admission_route(
            expected_kernel,
            historical_kernel,
            expected_capabilities,
        )? {
            RuntimeAdmissionRoute::Current => Ok(()),
            RuntimeAdmissionRoute::Historical | RuntimeAdmissionRoute::Invalid => {
                Err(TriggerValidationError::field(
                    "runtime_admission.kernel",
                    "does not name the selected durable runtime route",
                ))
            }
        }
    }

    pub fn update_overrides(
        &mut self,
        overrides: LaunchOverrides,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        let now = now.into();
        self.require_live_draft(&now)?;
        if self.started_at.is_some()
            || (self.state != LaunchState::Draft && self.state != LaunchState::AuthRequired)
        {
            return Err(TriggerValidationError::new(
                "overrides can change only before Start",
            ));
        }
        self.effective = effective_configuration(&self.revision, &overrides)?;
        self.pricing = None;
        self.runtime_admission = None;
        self.review_quota = None;
        self.start_admission = None;
        self.overrides = overrides;
        self.updated_at = now.clone();
        self.push_event(now, correlation_id, LaunchEventKind::DraftUpdated)
    }

    /// Freeze server-owned pricing, runtime, and quota evidence into the review.
    pub fn apply_review_admission(
        &mut self,
        admission: LaunchReviewAdmission,
    ) -> Result<(), TriggerValidationError> {
        if self.started_at.is_some()
            || !matches!(self.state, LaunchState::Draft | LaunchState::AuthRequired)
        {
            return Err(TriggerValidationError::new(
                "review admission can change only before Start",
            ));
        }
        validate_review_admission(self, &admission)?;
        self.pricing = Some(admission.pricing);
        self.runtime_admission = Some(admission.runtime);
        self.review_quota = Some(admission.quota);
        self.start_admission = None;
        self.effective.digest = effective_digest(
            &self.revision,
            &self.effective,
            self.pricing.as_ref(),
            self.runtime_admission.as_ref(),
        )?;
        Ok(())
    }

    pub fn record_start_admission(
        &mut self,
        admission: StartAdmissionSnapshot,
    ) -> Result<(), TriggerValidationError> {
        if self.state != LaunchState::Draft {
            return Err(TriggerValidationError::new(
                "Start admission can be recorded only for a draft",
            ));
        }
        let pricing = self.pricing.as_ref().ok_or_else(|| {
            TriggerValidationError::field("pricing", "the server quote is missing")
        })?;
        let runtime = self.runtime_admission.as_ref().ok_or_else(|| {
            TriggerValidationError::field(
                "runtime_admission",
                "the server capability admission is missing",
            )
        })?;
        if admission.runtime_version != runtime.runtime_version
            || admission.pricing_policy_reference != pricing.policy_reference
            || admission.rate_card != pricing.rate_card
        {
            return Err(TriggerValidationError::field(
                "start_admission",
                "does not revalidate the reviewed pricing and runtime",
            ));
        }
        self.start_admission = Some(admission);
        Ok(())
    }

    pub fn draft_is_expired(&self, now: &str) -> Result<bool, TriggerValidationError> {
        let now = crate::seconds_from_rfc3339(now)
            .ok_or_else(|| TriggerValidationError::field("now", "is not a CMA timestamp"))?;
        let expires = crate::seconds_from_rfc3339(&self.draft_expires_at).ok_or_else(|| {
            TriggerValidationError::field(
                "draft_expires_at",
                "is missing or is not a CMA timestamp",
            )
        })?;
        Ok(now >= expires)
    }

    pub fn require_live_draft(&self, now: &str) -> Result<(), TriggerValidationError> {
        if self.started_at.is_none()
            && matches!(self.state, LaunchState::Draft | LaunchState::AuthRequired)
            && self.draft_is_expired(now)?
        {
            return Err(TriggerValidationError::field(
                "draft_expires_at",
                "this Launch draft expired; open a New Launch to continue",
            ));
        }
        Ok(())
    }

    pub fn expire_draft(
        &mut self,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        let now = now.into();
        if self.started_at.is_some()
            || !matches!(self.state, LaunchState::Draft | LaunchState::AuthRequired)
        {
            return Ok(());
        }
        if !self.draft_is_expired(&now)? {
            return Err(TriggerValidationError::field(
                "draft_expires_at",
                "the Launch draft has not expired",
            ));
        }
        let correlation_id = correlation_id.into();
        self.fail(
            LaunchFailure {
                class: FailureClass::PolicyUserAction,
                code: "launch_draft_expired".into(),
                detail: "The saved Launch review expired before Start.".into(),
                remediation: "Open the README link again to create a New Launch with current policy, pricing, quota, and runtime evidence.".into(),
                correlation_id,
                retry_at: None,
                observed_at: now.clone(),
            },
            now,
        )
    }

    /// Record the explicit warning accepted before replacing a current Launch.
    ///
    /// The immutable revision digest is the snapshot for this action: a New
    /// Launch has no editable effective configuration until after it exists.
    pub fn acknowledge_new_launch(
        &mut self,
        subject: SubjectId,
        confirmed_at: impl Into<String>,
        idempotency_key_hash: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        if self.state != LaunchState::Draft || subject != self.owner {
            return Err(TriggerValidationError::field(
                "confirmation",
                "does not authorize a separate Launch for this subject",
            ));
        }
        let confirmed_at = confirmed_at.into();
        let confirmation = Confirmation {
            action: ConfirmationAction::NewLaunch,
            subject,
            snapshot_digest: self.revision_digest.clone(),
            confirmed_at: confirmed_at.clone(),
            idempotency_key_hash: idempotency_key_hash.into(),
        };
        self.confirmations.push(confirmation);
        self.updated_at = confirmed_at.clone();
        self.push_event(
            confirmed_at,
            correlation_id,
            LaunchEventKind::ConfirmationRecorded {
                action: ConfirmationAction::NewLaunch,
            },
        )
    }

    pub fn select_connection(
        &mut self,
        connection: ConnectionSnapshot,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        let mut candidate = self.clone();
        let now = now.into();
        candidate.require_live_draft(&now)?;
        if !matches!(
            candidate.state,
            LaunchState::Draft | LaunchState::AuthRequired
        ) {
            return Err(TriggerValidationError::new(
                "the connection can change only before Start or during authentication recovery",
            ));
        }
        connection.validate_for(
            &candidate
                .revision
                .content
                .repository_access
                .required_scope_classes,
        )?;
        let correlation_id = correlation_id.into();
        let recovery_target = (candidate.state == LaunchState::AuthRequired)
            .then(|| candidate.resume_state.unwrap_or(LaunchState::Draft));
        candidate.connection = Some(connection);
        if recovery_target.is_none() {
            candidate.blocked_reason = None;
        }
        candidate.updated_at = now.clone();
        candidate.push_event(
            now.clone(),
            correlation_id.clone(),
            LaunchEventKind::ConnectionSelected,
        )?;
        if let Some(target) = recovery_target {
            candidate.requeue_authentication_effect(target, &now, &correlation_id)?;
            candidate.failure = None;
            candidate.resume_state = None;
            candidate.transition(target, now, correlation_id)?;
        }
        *self = candidate;
        Ok(())
    }

    /// Put only the effect interrupted by authentication back on its original
    /// intent for read-only reconciliation. A capability renewal can also
    /// require authentication while no ordinary effect is pending, in which
    /// case returning to the retained state is sufficient.
    fn requeue_authentication_effect(
        &mut self,
        resume: LaunchState,
        now: &str,
        correlation_id: &str,
    ) -> Result<(), TriggerValidationError> {
        let mut resumed = self.clone();
        resumed.state = resume;
        resumed.blocked_reason = None;
        resumed.failure = None;
        resumed.resume_state = None;
        let Some(kind) = effect::required_effect(&resumed) else {
            return Ok(());
        };
        let index = self
            .effects
            .iter()
            .rposition(|record| record.kind == kind && record.state != EffectState::Succeeded)
            .ok_or_else(|| {
                TriggerValidationError::field(
                    "effect",
                    "authentication recovery is missing the exact retained failed intent",
                )
            })?;
        let retained_failure = self.failure.as_ref().ok_or_else(|| {
            TriggerValidationError::field(
                "failure",
                "authentication recovery must retain its failure classification",
            )
        })?;
        let record = &self.effects[index];
        if record.state != EffectState::FailedRetryable
            || record.last_failure.as_ref() != Some(retained_failure)
            || !matches!(
                retained_failure.class,
                FailureClass::AuthRequired
                    | FailureClass::OwnerCredentialUnavailable
                    | FailureClass::ServiceInactive
            )
        {
            return Err(TriggerValidationError::field(
                "effect",
                "authentication recovery does not match the exact retained failed intent",
            ));
        }

        let record = &mut self.effects[index];
        record.state = EffectState::Reconciling;
        record.next_attempt_at = None;
        record.updated_at = now.into();
        let step = self.step_mut(kind)?;
        step.status = LaunchStepStatus::Pending;
        step.safe_detail = "GitHub authority was revalidated; the retained operation is queued for read-only reconciliation.".into();
        step.remediation.clear();
        step.correlation_id = correlation_id.into();
        let step = step.clone();
        self.push_event(now, correlation_id, LaunchEventKind::StepChanged { step })
    }

    pub fn start(
        &mut self,
        confirmation: Confirmation,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        let now = now.into();
        self.require_live_draft(&now)?;
        if self.state != LaunchState::Draft {
            return Err(TriggerValidationError::new(
                "this Launch has already been started or is not startable",
            ));
        }
        if self.revision.availability != super::TriggerAvailability::Published {
            return Err(TriggerValidationError::new(
                "this Trigger revision is retired",
            ));
        }
        self.revision.content.validate()?;
        if self.revision.content.digest()? != self.revision_digest {
            return Err(TriggerValidationError::new(
                "the captured revision digest no longer matches its content",
            ));
        }
        super::validate_sha("source_sha", &self.source_sha)?;
        let connection = self.connection.as_ref().ok_or_else(|| {
            TriggerValidationError::field("connection", "select an eligible connection")
        })?;
        connection.validate_for(
            &self
                .revision
                .content
                .repository_access
                .required_scope_classes,
        )?;
        if !self.workflow_paths.is_empty() && !self.workflow_risk_acknowledged {
            return Err(TriggerValidationError::field(
                "workflow_risk_acknowledged",
                "must be confirmed before a reused fork can run repository workflows",
            ));
        }
        if self.pricing.is_none()
            || self.runtime_admission.is_none()
            || self.review_quota.is_none()
            || self.start_admission.is_none()
        {
            return Err(TriggerValidationError::field(
                "start_admission",
                "server pricing, quota, and runtime admission must be current before Start",
            ));
        }
        if confirmation.action != ConfirmationAction::Start
            || confirmation.subject != self.owner
            || confirmation.snapshot_digest != self.effective.digest
        {
            return Err(TriggerValidationError::field(
                "confirmation",
                "does not authorize this exact effective configuration",
            ));
        }
        self.confirmations.push(confirmation);
        self.started_at = Some(now.clone());
        self.push_event(now.clone(), correlation_id.into(), LaunchEventKind::Started)?;
        self.transition(LaunchState::Preflight, now, "start")
    }

    /// Record the contributor's explicit acceptance of the workflow paths
    /// shown for this exact reviewed Launch snapshot.
    pub fn acknowledge_workflow_risk(
        &mut self,
        confirmation: Confirmation,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        self.require_live_draft(&confirmation.confirmed_at)?;
        if self.state != LaunchState::Draft
            || self.workflow_paths.is_empty()
            || confirmation.action != ConfirmationAction::WorkflowRisk
            || confirmation.subject != self.owner
            || confirmation.snapshot_digest != self.effective.digest
        {
            return Err(TriggerValidationError::field(
                "workflow_risk_acknowledged",
                "does not acknowledge the reviewed workflow paths",
            ));
        }
        self.workflow_risk_acknowledged = true;
        self.updated_at = confirmation.confirmed_at.clone();
        self.confirmations.push(confirmation.clone());
        self.push_event(
            confirmation.confirmed_at,
            correlation_id,
            LaunchEventKind::ConfirmationRecorded {
                action: ConfirmationAction::WorkflowRisk,
            },
        )
    }

    pub fn transition(
        &mut self,
        to: LaunchState,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        let from = self.state;
        if from == to {
            return Ok(());
        }
        if from == LaunchState::AuthRequired
            && self.resume_state.is_some()
            && self.resume_state != Some(to)
        {
            return Err(TriggerValidationError::new(
                "authentication recovery must return to the recorded durable step",
            ));
        }
        if !legal_transition(from, to) {
            return Err(TriggerValidationError::new(format!(
                "Launch cannot transition from {from:?} to {to:?}"
            )));
        }
        validate_prerequisites(self, to)?;
        let correlation_id = correlation_id.into();
        self.state = to;
        self.updated_at = now.into();
        self.blocked_reason = None;
        if to.is_terminal() {
            self.terminal_at = Some(self.updated_at.clone());
        }
        self.push_event(
            self.updated_at.clone(),
            correlation_id.clone(),
            LaunchEventKind::StateChanged { from, to },
        )?;
        capability::request_after_terminal_transition(
            self,
            to,
            &self.updated_at.clone(),
            &correlation_id,
        )
    }

    pub fn block(
        &mut self,
        reason: LaunchBlockedReason,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        if self.blocked_reason == Some(reason) {
            return Ok(());
        }
        self.blocked_reason = Some(reason);
        self.updated_at = now.into();
        self.push_event(
            self.updated_at.clone(),
            correlation_id,
            LaunchEventKind::Blocked { reason },
        )
    }

    /// Fence work that depended on a reusable provider connection which the
    /// owner has revoked. Existing user artifacts remain attached to the
    /// Launch; reconnecting resumes through read-only effect reconciliation.
    pub fn require_connection_reauthentication(
        &mut self,
        service_id: &str,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<bool, TriggerValidationError> {
        if service_id.trim().is_empty() {
            return Err(TriggerValidationError::field(
                "connection.service_id",
                "the revoked connection identity is empty",
            ));
        }
        if self
            .connection
            .as_ref()
            .is_none_or(|connection| connection.service_id != service_id)
        {
            return Ok(false);
        }

        let before = self.clone();
        let now = now.into();
        let correlation_id = correlation_id.into();
        if self.state.is_terminal() {
            let reach_needs_revocation = self.reach.as_ref().is_some_and(|reach| {
                !matches!(
                    reach.state,
                    CapabilityRevocationState::RevocationPending
                        | CapabilityRevocationState::Revoked
                )
            });
            if self.lease.is_some() || reach_needs_revocation {
                self.lease = None;
                self.fencing_counter = self.fencing_counter.checked_add(1).ok_or_else(|| {
                    TriggerValidationError::field(
                        "fencing_counter",
                        "the Launch worker fence is exhausted",
                    )
                })?;
            }
            self.request_capability_revocation(&now, &correlation_id)?;
            return Ok(*self != before);
        }

        let retained_failure = self
            .failure
            .as_ref()
            .filter(|failure| failure.code == "trigger_connection_revoked")
            .cloned();
        let already_recorded =
            self.state == LaunchState::AuthRequired && retained_failure.is_some();
        let failure = retained_failure.unwrap_or_else(|| LaunchFailure {
            class: FailureClass::ServiceInactive,
            code: "trigger_connection_revoked".into(),
            detail: "The selected provider connection was revoked by its owner.".into(),
            remediation: "Reconnect or select an eligible connection to resume this Launch.".into(),
            correlation_id: correlation_id.clone(),
            retry_at: None,
            observed_at: now.clone(),
        });

        let required_effect = if self.state == LaunchState::AuthRequired {
            let mut resumed = self.clone();
            resumed.state = self.resume_state.unwrap_or(LaunchState::Draft);
            resumed.blocked_reason = None;
            resumed.failure = None;
            effect::required_effect(&resumed)
        } else {
            effect::required_effect(self)
        };
        let retained_effect_matches = required_effect.is_none_or(|kind| {
            self.effects
                .iter()
                .rfind(|record| {
                    record.kind == kind
                        && !matches!(
                            record.state,
                            EffectState::Succeeded | EffectState::Cancelled
                        )
                })
                .is_none_or(|record| {
                    record.state == EffectState::FailedRetryable
                        && record.last_failure.as_ref() == Some(&failure)
                        && record.authority_admission.is_none()
                })
        });
        let reach_needs_revocation = self.reach.as_ref().is_some_and(|reach| {
            !matches!(
                reach.state,
                CapabilityRevocationState::RevocationPending | CapabilityRevocationState::Revoked
            )
        });
        if self.lease.is_some()
            || !already_recorded
            || !retained_effect_matches
            || reach_needs_revocation
        {
            self.lease = None;
            self.fencing_counter = self.fencing_counter.checked_add(1).ok_or_else(|| {
                TriggerValidationError::field(
                    "fencing_counter",
                    "the Launch worker fence is exhausted",
                )
            })?;
        }

        if !retained_effect_matches {
            if let Some(kind) = required_effect {
                if let Some(record) = self.effects.iter_mut().rfind(|record| {
                    record.kind == kind
                        && !matches!(
                            record.state,
                            EffectState::Succeeded | EffectState::Cancelled
                        )
                }) {
                    // Reauthentication is allowed to resume only by reconciling this
                    // exact retained intent. It never turns a revoked-authority write
                    // back into a fresh execute attempt.
                    record.state = EffectState::FailedRetryable;
                    record.authority_admission = None;
                    record.next_attempt_at = None;
                    record.last_failure = Some(failure.clone());
                    record.updated_at = now.clone();
                }
            }
        }

        if !already_recorded {
            self.fail(failure, &now)?;
        }
        self.request_capability_revocation(&now, &correlation_id)?;
        Ok(*self != before)
    }

    /// Resume Trigger orchestration after the deployment-owned pause clears.
    /// The session and its reach are not restarted or recreated by this action.
    pub fn resume_feature_pause(
        &mut self,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<bool, TriggerValidationError> {
        if self.blocked_reason != Some(LaunchBlockedReason::FeaturePaused) {
            return Ok(false);
        }
        self.blocked_reason = None;
        if self
            .failure
            .as_ref()
            .is_some_and(|failure| failure.class == FailureClass::FeaturePaused)
        {
            self.failure = None;
        }
        let now = now.into();
        self.updated_at = now.clone();
        self.push_event(
            now,
            correlation_id,
            LaunchEventKind::Resumed {
                reason: LaunchBlockedReason::FeaturePaused,
            },
        )?;
        Ok(true)
    }

    pub fn fail(
        &mut self,
        failure: LaunchFailure,
        now: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        let (target, blocked_reason) = match failure.class {
            FailureClass::AuthRequired => (LaunchState::AuthRequired, None),
            FailureClass::OwnerCredentialUnavailable => {
                return self.apply_blocking_failure(
                    failure,
                    LaunchState::AuthRequired,
                    LaunchBlockedReason::OwnerCredentialUnavailable,
                    now,
                );
            }
            FailureClass::ServiceInactive => {
                return self.apply_blocking_failure(
                    failure,
                    LaunchState::AuthRequired,
                    LaunchBlockedReason::ServiceInactive,
                    now,
                );
            }
            FailureClass::RetryableTransient
            | FailureClass::AmbiguousExternal
            | FailureClass::ConflictStaleHead
            | FailureClass::ResourceQuota => (LaunchState::FailedRetryable, None),
            FailureClass::RecoveryRequired => {
                return self.apply_blocking_failure(
                    failure,
                    LaunchState::FailedRetryable,
                    LaunchBlockedReason::RecoveryRequired,
                    now,
                );
            }
            FailureClass::FeaturePaused => {
                return self.apply_blocking_failure(
                    failure,
                    self.state,
                    LaunchBlockedReason::FeaturePaused,
                    now,
                );
            }
            FailureClass::PolicyUserAction
            | FailureClass::TerminalValidation
            | FailureClass::TerminalSecurity => (LaunchState::FailedTerminal, None),
        };
        if !matches!(
            self.state,
            LaunchState::FailedRetryable | LaunchState::AuthRequired
        ) {
            self.resume_state = Some(self.state);
        }
        self.failure = Some(failure.clone());
        let now = now.into();
        if self.state != target {
            self.transition(target, now.clone(), failure.correlation_id.clone())?;
        }
        self.blocked_reason = blocked_reason;
        self.push_event(
            now,
            failure.correlation_id.clone(),
            LaunchEventKind::Failure { failure },
        )
    }

    fn apply_blocking_failure(
        &mut self,
        failure: LaunchFailure,
        target: LaunchState,
        reason: LaunchBlockedReason,
        now: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        if !matches!(
            self.state,
            LaunchState::FailedRetryable | LaunchState::AuthRequired
        ) && self.state != target
        {
            self.resume_state = Some(self.state);
        }
        self.failure = Some(failure.clone());
        let now = now.into();
        if self.state != target {
            self.transition(target, now.clone(), failure.correlation_id.clone())?;
        }
        self.blocked_reason = Some(reason);
        self.updated_at = now.clone();
        self.push_event(
            now,
            failure.correlation_id.clone(),
            LaunchEventKind::Failure { failure },
        )
    }

    pub fn cancel(
        &mut self,
        confirmation: Confirmation,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        if self.state.is_terminal() {
            return Ok(());
        }
        if confirmation.action != ConfirmationAction::Cancel
            || confirmation.subject != self.owner
            || confirmation.snapshot_digest != self.effective.digest
        {
            return Err(TriggerValidationError::field(
                "confirmation",
                "does not authorize cancellation of this exact Launch snapshot",
            ));
        }
        let now = now.into();
        let correlation_id = correlation_id.into();
        if self.effects.iter().any(|effect| {
            matches!(
                effect.state,
                EffectState::Claimed | EffectState::Reconciling | EffectState::RecoveryRequired
            )
        }) {
            return Err(TriggerValidationError::new(
                "unsettled external effects must be reconciled before cancellation",
            ));
        }
        self.confirmations.push(confirmation);
        for effect in &mut self.effects {
            if matches!(
                effect.state,
                EffectState::Queued | EffectState::FailedRetryable
            ) {
                effect.state = EffectState::Cancelled;
                effect.next_attempt_at = None;
                effect.updated_at = now.clone();
            }
        }
        let mut cancelled_steps = Vec::new();
        for step in &mut self.steps {
            if matches!(
                step.status,
                LaunchStepStatus::Pending
                    | LaunchStepStatus::Running
                    | LaunchStepStatus::Blocked
                    | LaunchStepStatus::Retryable
            ) {
                step.status = LaunchStepStatus::Cancelled;
                step.safe_detail = "Cancelled before this step completed.".into();
                step.remediation.clear();
                step.correlation_id = correlation_id.clone();
                cancelled_steps.push(step.clone());
            }
        }
        for step in cancelled_steps {
            self.push_event(
                now.clone(),
                correlation_id.clone(),
                LaunchEventKind::StepChanged { step },
            )?;
        }
        if self.state == LaunchState::PrPending && self.pull_request.is_some() {
            self.transition(LaunchState::PrOpen, now.clone(), correlation_id.clone())?;
        }
        self.transition(LaunchState::Cancelled, now.clone(), correlation_id.clone())?;
        self.push_event(now, correlation_id, LaunchEventKind::Cancelled)
    }

    /// End CMA orchestration after an open pull request has been handed to its
    /// maintainers. The provider pull request and contributor-owned Git state
    /// remain untouched; the terminal transition starts normal reach
    /// revocation and retention handling.
    pub fn complete(
        &mut self,
        confirmation: Confirmation,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        if self.state == LaunchState::Completed {
            return Ok(());
        }
        if self.state != LaunchState::PrOpen
            || self.pull_request.as_ref().is_none_or(|pull_request| {
                !matches!(
                    pull_request.state,
                    PullRequestState::Open | PullRequestState::Merged
                )
            })
        {
            return Err(TriggerValidationError::field(
                "confirmation",
                "can complete only after CMA has handed off an open pull request",
            ));
        }
        if confirmation.action != ConfirmationAction::Complete
            || confirmation.subject != self.owner
            || confirmation.snapshot_digest != self.effective.digest
        {
            return Err(TriggerValidationError::field(
                "confirmation",
                "does not authorize completion of this exact Launch snapshot",
            ));
        }
        let now = now.into();
        let correlation_id = correlation_id.into();
        self.confirmations.push(confirmation);
        self.push_event(
            now.clone(),
            correlation_id.clone(),
            LaunchEventKind::ConfirmationRecorded {
                action: ConfirmationAction::Complete,
            },
        )?;
        self.transition(LaunchState::Completed, now, correlation_id)
    }

    /// Whether cancellation still owes the retained session a durable
    /// first-turn release marker.
    ///
    /// `Completed` and `Failed` both release owner input. A `Failed` delivery
    /// with attempt zero and no fence is the distinguished pre-dispatch
    /// cancellation tombstone; an attempted runtime failure has attempt and
    /// fence evidence instead.
    pub fn cancelled_session_requires_settlement(&self) -> bool {
        self.state == LaunchState::Cancelled
            && self.session.is_some()
            && self.delivery.as_ref().is_none_or(|delivery| {
                !matches!(
                    delivery.state,
                    PromptDeliveryState::Completed | PromptDeliveryState::Failed
                )
            })
    }

    /// Mirror positive session-store settlement into the Launch projection.
    pub fn record_cancelled_session_settlement(
        &mut self,
        delivery: PromptDelivery,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        if self.state != LaunchState::Cancelled
            || !matches!(
                delivery.state,
                PromptDeliveryState::Completed | PromptDeliveryState::Failed
            )
        {
            return Err(TriggerValidationError::field(
                "delivery",
                "does not settle a cancelled Trigger session",
            ));
        }
        self.observe_prompt_delivery(delivery, now.into(), correlation_id.into())
    }

    /// Begin or refresh a read-only contribution review.
    ///
    /// Every request selects the next checkpoint version before any worker is
    /// allowed to capture it. The later publication confirmation is separate
    /// and binds the resulting diff digest, so this action never authorizes a
    /// GitHub write.
    pub fn request_diff_review(
        &mut self,
        confirmation: Confirmation,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        if !matches!(
            self.state,
            LaunchState::Ready | LaunchState::Contributing | LaunchState::PrOpen
        ) {
            return Err(TriggerValidationError::new(
                "a diff can be reviewed only from a ready session or its open pull request",
            ));
        }
        match self.pull_request.as_ref() {
            Some(pull_request)
                if pull_request.state != super::PullRequestState::Open
                    || pull_request.status_sync.head_matches_launch != Some(true) =>
            {
                return Err(TriggerValidationError::field(
                    "pull_request",
                    "must remain open and match the retained Launch head before another contribution review",
                ));
            }
            None if self.state == LaunchState::PrOpen => {
                return Err(TriggerValidationError::field(
                    "pull_request",
                    "is missing from a Launch linked to an open pull request",
                ));
            }
            _ => {}
        }
        if confirmation.action != ConfirmationAction::ReviewDiff
            || confirmation.subject != self.owner
            || confirmation.snapshot_digest != self.effective.digest
        {
            return Err(TriggerValidationError::field(
                "confirmation",
                "does not request a review for this exact Launch configuration",
            ));
        }
        let session = self.session.as_ref().ok_or_else(|| {
            TriggerValidationError::field(
                "checkpoint_intent.session_id",
                "cannot review a diff before the deterministic session exists",
            )
        })?;
        let workspace = self.workspace.as_ref().ok_or_else(|| {
            TriggerValidationError::field(
                "checkpoint_intent",
                "cannot review a diff before workspace import",
            )
        })?;
        if workspace.checkpoint_intent.is_some() {
            return Err(TriggerValidationError::field(
                "checkpoint_intent",
                "another coherent workspace capture is still settling; retry this review",
            ));
        }
        let current = workspace
            .latest_checkpoint
            .as_ref()
            .map_or(workspace.checkpoint_version, |checkpoint| {
                workspace.checkpoint_version.max(checkpoint.version)
            });
        let requested = current.checked_add(1).ok_or_else(|| {
            TriggerValidationError::field(
                "contribution_checkpoint_version",
                "cannot advance beyond the retained checkpoint",
            )
        })?;
        let now = now.into();
        let correlation_id = correlation_id.into();
        self.contribution_checkpoint_version = Some(requested);
        self.workspace
            .as_mut()
            .expect("validated above")
            .checkpoint_intent = Some(super::WorkspaceCheckpointIntent {
            id: CheckpointId::new(),
            version: requested,
            purpose: super::WorkspaceCheckpointPurpose::ContributionReview,
            session_id: session.session_id.clone(),
            runtime_generation: session.runtime_generation,
            workspace_epoch: workspace.workspace_epoch,
            requested_at: now.clone(),
            retry_at: None,
        });
        self.diff = None;
        self.contribution = None;
        self.confirmations.push(confirmation.clone());
        self.updated_at = now.clone();
        self.push_event(
            now.clone(),
            correlation_id.clone(),
            LaunchEventKind::ConfirmationRecorded {
                action: ConfirmationAction::ReviewDiff,
            },
        )?;
        if self.state != LaunchState::Contributing {
            self.transition(LaunchState::Contributing, now, correlation_id)?;
        }
        Ok(())
    }

    /// Record the explicit authority to publish exactly one reviewed diff.
    ///
    /// This creates the durable write intent before a worker can make any Git
    /// Data request. Pull-request creation remains a separate confirmation and
    /// therefore has no required effect after branch publication completes.
    pub fn confirm_branch_publication(
        &mut self,
        confirmation: Confirmation,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        if self.state != LaunchState::Contributing {
            return Err(TriggerValidationError::new(
                "a branch can be published only after contribution review",
            ));
        }
        let diff = self.diff.as_ref().ok_or_else(|| {
            TriggerValidationError::field("diff", "must be reviewed before publication")
        })?;
        diff.validate_for_publication(self)?;
        if confirmation.action != ConfirmationAction::PublishBranch
            || confirmation.subject != self.owner
            || confirmation.snapshot_digest != diff.digest
        {
            return Err(TriggerValidationError::field(
                "confirmation",
                "does not authorize publication of this exact reviewed diff",
            ));
        }
        if self.contribution.is_some() {
            return Err(TriggerValidationError::field(
                "contribution",
                "already has a publication intent",
            ));
        }
        let branch = self.branch.as_ref().ok_or_else(|| {
            TriggerValidationError::field("branch", "is missing from the reviewed Launch")
        })?;
        let now = now.into();
        let correlation_id = correlation_id.into();
        self.contribution = Some(super::ContributionRecord {
            id: super::ContributionId::new(),
            launch_id: self.id.clone(),
            diff_artifact_id: diff.id.clone(),
            revision_digest: self.revision_digest.clone(),
            baseline_digest: diff.baseline_digest.clone(),
            checkpoint_digest: diff.checkpoint_digest.clone(),
            diff_digest: diff.digest.clone(),
            branch_name: branch.name.clone(),
            state: super::PublicationState::IntentRecorded,
            expected_head: diff.expected_branch_head.clone(),
            base_tree_id: None,
            tree_id: None,
            commit_id: None,
            resulting_head: None,
            object_ids: Vec::new(),
            object_write: None,
            author: diff.author.clone(),
            committer: diff.committer.clone(),
            confirmation: confirmation.clone(),
            intent_key: format!("cma-trigger:{}:publish:{}", self.id.0, diff.id.0),
            created_at: now.clone(),
            updated_at: now.clone(),
        });
        self.contribution
            .as_ref()
            .expect("created above")
            .validate_intent(&self.id)?;
        self.confirmations.push(confirmation);
        self.updated_at = now.clone();
        self.push_event(
            now.clone(),
            correlation_id.clone(),
            LaunchEventKind::ConfirmationRecorded {
                action: ConfirmationAction::PublishBranch,
            },
        )?;
        self.transition(LaunchState::PrPending, now, correlation_id)
    }

    /// Build the exact pull-request target and content shown for contributor
    /// review. This performs no provider call and grants no write authority.
    pub fn pull_request_draft(
        &self,
        operation: super::PullRequestOperation,
        title: Option<&str>,
        body: Option<&str>,
    ) -> Result<super::PullRequestDraft, TriggerValidationError> {
        if let Some(existing) = self.pull_request.as_ref() {
            if existing.intent.operation == operation
                && matches!(
                    existing.state,
                    super::PullRequestState::Creating
                        | super::PullRequestState::Updating
                        | super::PullRequestState::Open
                        | super::PullRequestState::ClosedUnmerged
                        | super::PullRequestState::Merged
                        | super::PullRequestState::RecoveryRequired
                )
                && (matches!(
                    existing.state,
                    super::PullRequestState::Creating
                        | super::PullRequestState::Updating
                        | super::PullRequestState::RecoveryRequired
                ) || self
                    .contribution
                    .as_ref()
                    .and_then(|record| record.resulting_head.as_deref())
                    == Some(existing.intent.expected_head_revision.as_str()))
                && title.is_none()
                && body.is_none()
            {
                return Ok(existing.intent.clone());
            }
        }
        if self.state != LaunchState::PrPending {
            return Err(TriggerValidationError::new(
                "a pull request can be reviewed only after guarded branch publication",
            ));
        }
        let contribution = self.contribution.as_ref().ok_or_else(|| {
            TriggerValidationError::field(
                "contribution",
                "the Launch has no reviewed branch publication",
            )
        })?;
        if contribution.state != super::PublicationState::Published {
            return Err(TriggerValidationError::field(
                "contribution",
                "the reviewed branch must be published before PR review",
            ));
        }
        let head_revision = contribution.resulting_head.clone().ok_or_else(|| {
            TriggerValidationError::field(
                "contribution.resulting_head",
                "the published commit is missing",
            )
        })?;
        let branch = self.branch.as_ref().ok_or_else(|| {
            TriggerValidationError::field("branch", "the Launch branch is missing")
        })?;
        if branch.name != contribution.branch_name || branch.current_head != head_revision {
            return Err(TriggerValidationError::field(
                "branch",
                "does not point at the exact reviewed publication",
            ));
        }
        let connection = self.connection.as_ref().ok_or_else(|| {
            TriggerValidationError::field("connection", "the GitHub connection is missing")
        })?;
        let fork = self.fork.as_ref().ok_or_else(|| {
            TriggerValidationError::field("fork", "the reconciled fork is missing")
        })?;
        if fork.repository.owner_id != connection.account.account_id
            || fork.repository.owner_node_id != connection.account.account_node_id
        {
            return Err(TriggerValidationError::field(
                "fork",
                "is not owned by the selected contributor account",
            ));
        }

        let generated_title = self
            .revision
            .content
            .contribution
            .pull_request
            .title_template
            .trim()
            .to_string();
        let generated_body = format!(
            "{}\n\nCMA Launch: `{}`\nTrigger revision: `{}`\nReviewed commit: `{}`",
            self.revision
                .content
                .contribution
                .pull_request
                .body_template
                .trim(),
            self.id.0,
            self.revision_digest,
            head_revision
        );
        let (
            previous_head_revision,
            target_number,
            target_provider_id,
            target_provider_node_id,
            update_metadata,
            expected_previous_title,
            expected_previous_body_digest,
            approved_title,
            approved_body,
        ) = match operation {
            super::PullRequestOperation::Open => {
                if self.pull_request.is_some() {
                    return Err(TriggerValidationError::field(
                        "pull_request",
                        "this Launch is already linked to a provider pull request",
                    ));
                }
                (
                    None,
                    None,
                    None,
                    None,
                    false,
                    None,
                    None,
                    title.unwrap_or(&generated_title).to_string(),
                    body.unwrap_or(&generated_body).to_string(),
                )
            }
            super::PullRequestOperation::Update => {
                let existing = self.pull_request.as_ref().ok_or_else(|| {
                    TriggerValidationError::field(
                        "pull_request",
                        "an update requires an existing open pull request",
                    )
                })?;
                if existing.state != super::PullRequestState::Open {
                    return Err(TriggerValidationError::field(
                        "pull_request.state",
                        "closed or merged pull requests cannot be updated",
                    ));
                }
                let previous = existing.head_revision.clone().ok_or_else(|| {
                    TriggerValidationError::field(
                        "pull_request.head_revision",
                        "the previously observed provider head is missing",
                    )
                })?;
                if contribution.expected_head != previous {
                    return Err(TriggerValidationError::field(
                        "contribution.expected_head",
                        "does not continue the previously reviewed PR head",
                    ));
                }
                let update_metadata = title.is_some() || body.is_some();
                (
                    Some(previous),
                    existing.number,
                    existing.provider_id,
                    existing.provider_node_id.clone(),
                    update_metadata,
                    update_metadata.then(|| {
                        existing
                            .observed_title
                            .clone()
                            .expect("a validated open PR retains its observed title")
                    }),
                    update_metadata.then(|| {
                        existing
                            .observed_body_digest
                            .clone()
                            .expect("a validated open PR retains its observed body digest")
                    }),
                    title.unwrap_or(&existing.intent.title).to_string(),
                    body.unwrap_or(&existing.intent.body).to_string(),
                )
            }
        };
        super::PullRequestDraft::seal(
            self.id.clone(),
            operation,
            self.revision.content.upstream.repository.repository_id,
            self.revision
                .content
                .upstream
                .repository
                .repository_node_id
                .clone(),
            fork.repository.repository_id,
            fork.repository.repository_node_id.clone(),
            self.revision.content.upstream.contribution_base_ref.clone(),
            fork.repository.canonical_owner.clone(),
            branch.name.clone(),
            self.source_sha.clone(),
            head_revision,
            previous_head_revision,
            target_number,
            target_provider_id,
            target_provider_node_id,
            update_metadata,
            expected_previous_title,
            expected_previous_body_digest,
            approved_title,
            approved_body,
        )
    }

    /// Persist explicit Open/Update authority before a worker may touch the
    /// provider. Repeated confirmation of the same sealed intent is inert.
    pub fn confirm_pull_request(
        &mut self,
        confirmation: Confirmation,
        title: Option<&str>,
        body: Option<&str>,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        let operation = match confirmation.action {
            ConfirmationAction::OpenPullRequest => super::PullRequestOperation::Open,
            ConfirmationAction::UpdatePullRequest => super::PullRequestOperation::Update,
            _ => {
                return Err(TriggerValidationError::field(
                    "confirmation.action",
                    "must explicitly authorize opening or updating a pull request",
                ))
            }
        };
        if confirmation.subject != self.owner {
            return Err(TriggerValidationError::field(
                "confirmation.subject",
                "does not own this Launch",
            ));
        }
        if let Some(existing) = self.pull_request.as_ref() {
            if existing.intent.operation == operation
                && existing.intent.intent_digest == confirmation.snapshot_digest
                && (matches!(
                    existing.state,
                    super::PullRequestState::Creating
                        | super::PullRequestState::Updating
                        | super::PullRequestState::RecoveryRequired
                ) || self
                    .contribution
                    .as_ref()
                    .and_then(|record| record.resulting_head.as_deref())
                    == Some(existing.intent.expected_head_revision.as_str()))
                && title.is_none()
                && body.is_none()
            {
                return Ok(());
            }
        }
        let draft = self.pull_request_draft(operation, title, body)?;
        if confirmation.snapshot_digest != draft.intent_digest {
            return Err(TriggerValidationError::field(
                "confirmation.snapshot_digest",
                "does not authorize the exact reviewed PR target and content",
            ));
        }
        let now = now.into();
        let correlation_id = correlation_id.into();
        let mut record = match operation {
            super::PullRequestOperation::Open => super::PullRequestRecord {
                intent: draft,
                provider_id: None,
                provider_node_id: None,
                number: None,
                canonical_url: None,
                state: super::PullRequestState::Creating,
                base_revision: None,
                head_revision: None,
                observed_title: None,
                observed_body_digest: None,
                mergeability: super::Mergeability::Computing,
                checks: super::CheckSummaryState::Unavailable,
                confirmation: confirmation.clone(),
                intent_key: String::new(),
                created_at: confirmation.confirmed_at.clone(),
                updated_at: now.clone(),
                provider_created_at: None,
                provider_updated_at: None,
                status_sync: super::PullRequestStatusSync::default(),
            },
            super::PullRequestOperation::Update => {
                let existing = self
                    .pull_request
                    .as_ref()
                    .expect("a validated update has an existing provider PR");
                super::PullRequestRecord {
                    intent: draft,
                    provider_id: existing.provider_id,
                    provider_node_id: existing.provider_node_id.clone(),
                    number: existing.number,
                    canonical_url: existing.canonical_url.clone(),
                    state: super::PullRequestState::Updating,
                    base_revision: existing.base_revision.clone(),
                    head_revision: existing.head_revision.clone(),
                    observed_title: existing.observed_title.clone(),
                    observed_body_digest: existing.observed_body_digest.clone(),
                    mergeability: existing.mergeability,
                    checks: existing.checks,
                    confirmation: confirmation.clone(),
                    intent_key: String::new(),
                    created_at: confirmation.confirmed_at.clone(),
                    updated_at: now.clone(),
                    provider_created_at: existing.provider_created_at.clone(),
                    provider_updated_at: existing.provider_updated_at.clone(),
                    status_sync: super::PullRequestStatusSync::default(),
                }
            }
        };
        record.intent_key = format!(
            "cma-trigger:{}:pr:{}:{}",
            self.id.0,
            record.intent.operation.wire_name(),
            record.intent.intent_digest
        );
        record.validate_intent(&self.id)?;
        self.pull_request = Some(record);
        self.confirmations.push(confirmation.clone());
        self.updated_at = now.clone();
        self.push_event(
            now,
            correlation_id,
            LaunchEventKind::ConfirmationRecorded {
                action: confirmation.action,
            },
        )
    }

    pub fn events_after(&self, cursor: u64) -> Vec<LaunchEvent> {
        self.events
            .iter()
            .filter(|event| event.sequence > cursor)
            .cloned()
            .collect()
    }

    /// The durable stream position, including compatibility with Launches
    /// written before `event_sequence` was materialized separately.
    pub fn event_high_water(&self) -> u64 {
        self.event_sequence
            .max(self.events.last().map_or(0, |event| event.sequence))
    }

    pub fn next_effect(&self) -> Option<LaunchEffectKind> {
        if self.blocked_reason.is_some() || self.state.is_terminal() || self.lease.is_some() {
            return None;
        }
        effect::required_effect(self)
    }

    pub(super) fn push_event(
        &mut self,
        occurred_at: impl Into<String>,
        correlation_id: impl Into<String>,
        kind: LaunchEventKind,
    ) -> Result<(), TriggerValidationError> {
        let sequence = self
            .event_high_water()
            .checked_add(1)
            .ok_or_else(|| TriggerValidationError::new("Launch event sequence is exhausted"))?;
        self.event_sequence = sequence;
        self.events.push(LaunchEvent {
            sequence,
            launch_id: self.id.clone(),
            occurred_at: occurred_at.into(),
            correlation_id: correlation_id.into(),
            progress: LaunchProgressState::from_launch(self),
            kind,
        });
        Ok(())
    }
}

fn default_steps(correlation_id: &str) -> Vec<LaunchStep> {
    [
        LaunchStepKey::Review,
        LaunchStepKey::Authentication,
        LaunchStepKey::Preflight,
        LaunchStepKey::Fork,
        LaunchStepKey::Branch,
        LaunchStepKey::Workspace,
        LaunchStepKey::Sandbox,
        LaunchStepKey::Session,
        LaunchStepKey::Reach,
        LaunchStepKey::InitialPrompt,
        LaunchStepKey::Checkpoint,
        LaunchStepKey::Diff,
        LaunchStepKey::PublishBranch,
        LaunchStepKey::PullRequest,
        LaunchStepKey::Cleanup,
    ]
    .into_iter()
    .enumerate()
    .map(|(index, key)| LaunchStep {
        key,
        sequence: index as u32 + 1,
        status: LaunchStepStatus::Pending,
        started_at: None,
        completed_at: None,
        attempt: 0,
        safe_detail: String::new(),
        remediation: String::new(),
        external_resource_ids: BTreeMap::new(),
        worker_fencing_token: 0,
        correlation_id: correlation_id.to_string(),
    })
    .collect()
}

pub fn effective_configuration(
    revision: &TriggerRevision,
    overrides: &LaunchOverrides,
) -> Result<EffectiveConfiguration, TriggerValidationError> {
    revision.content.validate()?;
    let allowed: BTreeSet<_> = revision
        .content
        .override_policy
        .editable_fields
        .iter()
        .copied()
        .collect();
    if !overrides.task_inputs.is_empty() && !allowed.contains(&EditableField::TaskInput) {
        return Err(TriggerValidationError::field(
            "overrides.task_inputs",
            "task inputs are locked by the publisher",
        ));
    }
    let work_label = overrides
        .work_label
        .as_deref()
        .unwrap_or("work")
        .trim()
        .to_string();
    if overrides.work_label.is_some() && !allowed.contains(&EditableField::WorkLabel) {
        return Err(TriggerValidationError::field(
            "overrides.work_label",
            "the work label is locked by the publisher",
        ));
    }
    if work_label.is_empty()
        || work_label.len() > 48
        || !work_label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b'-' | b'_'))
    {
        return Err(TriggerValidationError::field(
            "overrides.work_label",
            "must contain only letters, numbers, spaces, dash, or underscore",
        ));
    }
    let resource_size = overrides
        .resource_size
        .unwrap_or(revision.content.session.resources.default);
    if overrides.resource_size.is_some() && !allowed.contains(&EditableField::ResourceSize) {
        return Err(TriggerValidationError::field(
            "overrides.resource_size",
            "resource size is locked by the publisher",
        ));
    }
    let ceiling = revision.content.session.resources.ceiling;
    if resource_size.cpu_millis() > ceiling.cpu_millis()
        || resource_size.memory_mib() > ceiling.memory_mib()
    {
        return Err(TriggerValidationError::field(
            "overrides.resource_size",
            "exceeds the publisher ceiling",
        ));
    }
    let timeout_seconds = overrides
        .timeout_seconds
        .unwrap_or(revision.content.session.timeout_default_seconds);
    if overrides.timeout_seconds.is_some() && !allowed.contains(&EditableField::Timeout) {
        return Err(TriggerValidationError::field(
            "overrides.timeout_seconds",
            "timeout is locked by the publisher",
        ));
    }
    if timeout_seconds == 0 || timeout_seconds > revision.content.session.timeout_ceiling_seconds {
        return Err(TriggerValidationError::field(
            "overrides.timeout_seconds",
            "exceeds the publisher ceiling",
        ));
    }
    if !overrides.optional_checks.is_empty() {
        return Err(TriggerValidationError::field(
            "overrides.optional_checks",
            "configured checks are unavailable in this deployment",
        ));
    }
    let rendered_prompt = revision.content.render_prompt(&overrides.task_inputs)?;
    let mut effective = EffectiveConfiguration {
        rendered_prompt,
        work_label,
        permission: revision.content.session.permission_ceiling,
        resource_size,
        timeout_seconds,
        optional_checks: Vec::new(),
        digest: String::new(),
    };
    effective.digest = effective_digest(revision, &effective, None, None)?;
    Ok(effective)
}

fn effective_digest(
    revision: &TriggerRevision,
    effective: &EffectiveConfiguration,
    pricing: Option<&PricingSnapshot>,
    runtime: Option<&RuntimeAdmissionSnapshot>,
) -> Result<String, TriggerValidationError> {
    let digest_input = serde_json::to_vec(&(
        &revision.digest,
        &effective.rendered_prompt,
        &effective.work_label,
        effective.permission,
        effective.resource_size,
        effective.timeout_seconds,
        &effective.optional_checks,
        pricing,
        runtime,
    ))
    .map_err(|_| TriggerValidationError::new("effective configuration cannot be encoded"))?;
    Ok(Sha256::digest(digest_input)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn draft_expiry(now: &str) -> Result<String, TriggerValidationError> {
    let created = crate::seconds_from_rfc3339(now)
        .filter(|seconds| *seconds > 0)
        .ok_or_else(|| TriggerValidationError::field("created_at", "is not a CMA timestamp"))?;
    let expires = created
        .checked_add(LAUNCH_DRAFT_TTL_SECONDS)
        .ok_or_else(|| {
            TriggerValidationError::field("draft_expires_at", "exceeds the supported clock")
        })?;
    Ok(crate::rfc3339_at(
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(expires),
    ))
}

fn validate_review_admission(
    launch: &Launch,
    admission: &LaunchReviewAdmission,
) -> Result<(), TriggerValidationError> {
    let pricing = &admission.pricing;
    let policy = &launch.revision.content.commercial_policy;
    if pricing.policy_reference != policy.policy_reference
        || pricing.billing_mode != policy.billing_mode
        || pricing.currency != crate::CURRENCY
        || pricing.cpu_millis != launch.effective.resource_size.cpu_millis()
        || pricing.memory_mib != launch.effective.resource_size.memory_mib()
        || crate::seconds_from_rfc3339(&pricing.quoted_at).is_none()
        || crate::seconds_from_rfc3339(&pricing.expires_at).is_none()
        || pricing.quoted_at >= pricing.expires_at
        || pricing.expires_at > launch.draft_expires_at
    {
        return Err(TriggerValidationError::field(
            "pricing",
            "does not describe this exact effective configuration and draft lifetime",
        ));
    }
    match pricing.billing_mode {
        super::BillingMode::ZeroCost
            if pricing.rate_card.is_none()
                && pricing.rate.is_none()
                && pricing.micros_per_hour == 0
                && pricing.estimated_micros == 0 => {}
        super::BillingMode::Contributor
            if pricing.rate_card.is_some()
                && pricing.rate.is_some()
                && pricing.micros_per_hour > 0 => {}
        _ => {
            return Err(TriggerValidationError::field(
                "pricing",
                "contains an unsupported or incomplete billing policy",
            ));
        }
    }

    let runtime = &admission.runtime;
    let supported: BTreeSet<_> = runtime.supported_capabilities.iter().copied().collect();
    if runtime.kernel.is_unset()
        || runtime.runtime_version.trim().is_empty()
        || runtime.supported_capabilities.len() != supported.len()
        || crate::seconds_from_rfc3339(&runtime.observed_at).is_none()
        || launch
            .revision
            .content
            .session
            .required_capabilities
            .iter()
            .any(|capability| !supported.contains(capability))
    {
        return Err(TriggerValidationError::field(
            "runtime_admission",
            "does not support the complete published capability set on a pinned runtime",
        ));
    }
    if crate::seconds_from_rfc3339(&admission.quota.checked_at).is_none() {
        return Err(TriggerValidationError::field(
            "review_quota",
            "does not contain a CMA timestamp",
        ));
    }
    Ok(())
}

pub(super) fn legal_transition(from: LaunchState, to: LaunchState) -> bool {
    use LaunchState::*;
    if matches!(to, FailedRetryable | FailedTerminal | Cancelled) && !from.is_terminal() {
        return true;
    }
    if to == AuthRequired && from.is_active() {
        return true;
    }
    if from == AuthRequired && to.is_active() {
        return true;
    }
    matches!(
        (from, to),
        (Draft, AuthRequired)
            | (AuthRequired, Draft)
            | (Draft, Preflight)
            | (AuthRequired, Preflight)
            | (Preflight, ForkPending)
            | (ForkPending, ForkReady)
            | (ForkReady, BranchPending)
            | (BranchPending, WorkspacePending)
            | (WorkspacePending, SandboxPending)
            | (SandboxPending, SessionPending)
            | (SessionPending, PromptPending)
            | (PromptPending, Ready)
            | (Ready, Contributing)
            | (Contributing, Ready)
            | (Contributing, PrPending)
            | (PrPending, PrOpen)
            | (PrOpen, Contributing)
            | (PrOpen, Completed)
            | (FailedRetryable, Preflight)
            | (FailedRetryable, ForkPending)
            | (FailedRetryable, ForkReady)
            | (FailedRetryable, BranchPending)
            | (FailedRetryable, WorkspacePending)
            | (FailedRetryable, SandboxPending)
            | (FailedRetryable, SessionPending)
            | (FailedRetryable, PromptPending)
            | (FailedRetryable, Ready)
            | (FailedRetryable, Contributing)
            | (FailedRetryable, PrPending)
    )
}

fn validate_prerequisites(launch: &Launch, to: LaunchState) -> Result<(), TriggerValidationError> {
    use LaunchState::*;
    if matches!(
        to,
        ForkPending
            | ForkReady
            | BranchPending
            | WorkspacePending
            | SandboxPending
            | SessionPending
            | PromptPending
            | Ready
            | Contributing
            | PrPending
            | PrOpen
            | Completed
    ) && launch.connection.is_none()
    {
        return Err(TriggerValidationError::new(
            "an authoritative connection snapshot is required",
        ));
    }
    if matches!(
        to,
        BranchPending
            | WorkspacePending
            | SandboxPending
            | SessionPending
            | PromptPending
            | Ready
            | Contributing
            | PrPending
            | PrOpen
            | Completed
    ) && launch.fork.is_none()
    {
        return Err(TriggerValidationError::new("a reconciled fork is required"));
    }
    if matches!(
        to,
        WorkspacePending
            | SandboxPending
            | SessionPending
            | PromptPending
            | Ready
            | Contributing
            | PrPending
            | PrOpen
            | Completed
    ) && launch.branch.is_none()
    {
        return Err(TriggerValidationError::new(
            "a fenced Launch branch is required",
        ));
    }
    if matches!(
        to,
        SandboxPending
            | SessionPending
            | PromptPending
            | Ready
            | Contributing
            | PrPending
            | PrOpen
            | Completed
    ) && launch.workspace.is_none()
    {
        return Err(TriggerValidationError::new(
            "a complete baseline workspace is required",
        ));
    }
    if matches!(
        to,
        PromptPending | Ready | Contributing | PrPending | PrOpen | Completed
    ) && launch.session.is_none()
    {
        return Err(TriggerValidationError::new("a linked session is required"));
    }
    if matches!(to, Ready | Contributing | PrPending | PrOpen | Completed)
        && (launch.reach.is_none() || launch.delivery.is_none())
    {
        return Err(TriggerValidationError::new(
            "session reach and initial delivery records are required",
        ));
    }
    if matches!(to, PrPending | PrOpen | Completed) && launch.diff.is_none() {
        return Err(TriggerValidationError::new("a reviewed diff is required"));
    }
    if matches!(to, PrOpen | Completed) && launch.pull_request.is_none() {
        return Err(TriggerValidationError::new(
            "a reconciled pull request is required",
        ));
    }
    Ok(())
}
