use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{
    AuthorityProvenance, ConnectionSnapshot, DiffArtifact, EditableField, LaunchId,
    RepositoryIdentity, TriggerRevision, TriggerValidationError, WorkspaceCheckpoint,
    WorkspaceManifest,
};
use crate::{PermissionMode, SessionId, SessionSize, SubjectId};

mod capability;
mod effect;

pub const LAUNCH_SCHEMA_VERSION: u32 = 1;
pub const MAX_LAUNCH_EVENTS: usize = 2_048;
pub const MAX_LAUNCH_EFFECT_ATTEMPTS: u32 = 5;
pub const MAX_LAUNCH_RECONCILIATION_ATTEMPTS: u32 = 20;

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
    Created,
    DraftUpdated,
    ConnectionSelected,
    Started,
    StateChanged { from: LaunchState, to: LaunchState },
    StepChanged { step: LaunchStep },
    Blocked { reason: LaunchBlockedReason },
    Failure { failure: LaunchFailure },
    EffectReconciled { effect: LaunchEffectKind },
    DeliveryChanged { state: PromptDeliveryState },
    ReachChanged { reach: super::ReachStatus },
    ConfirmationRecorded { action: ConfirmationAction },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LaunchEvent {
    pub sequence: u64,
    pub launch_id: LaunchId,
    pub occurred_at: String,
    pub correlation_id: String,
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

/// Typed durable result for one effect. Provider adapters cannot advance a
/// Launch by returning an unstructured success flag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
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
    Cleanup,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct LaunchOverrides {
    #[serde(default)]
    pub task_inputs: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_suffix: Option<String>,
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
    pub branch_suffix: String,
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
    pub currency: String,
    pub estimated_micros: u64,
    pub quoted_at: String,
    pub expires_at: String,
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
    pub dispatch_fence_sequence: Option<u64>,
    pub queued_at: String,
    pub updated_at: String,
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
    pub reach: Option<super::ReachStatus>,
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
    pub lease: Option<WorkerLease>,
    pub fencing_counter: u64,
    pub revision_number: u64,
    pub created_at: String,
    pub updated_at: String,
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
            connection: None,
            authority: None,
            workflow_paths: Vec::new(),
            workflow_risk_acknowledged: false,
            steps,
            effects: Vec::new(),
            events: Vec::new(),
            confirmations: Vec::new(),
            fork: None,
            branch: None,
            workspace: None,
            session: None,
            reach: None,
            delivery: None,
            contribution_checkpoint_version: None,
            diff: None,
            contribution: None,
            pull_request: None,
            lease: None,
            fencing_counter: 0,
            revision_number: 0,
            created_at: now.clone(),
            updated_at: now.clone(),
            started_at: None,
            terminal_at: None,
        };
        launch.push_event(now, correlation_id, LaunchEventKind::Created)?;
        Ok(launch)
    }

    pub fn update_overrides(
        &mut self,
        overrides: LaunchOverrides,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        if self.state != LaunchState::Draft && self.state != LaunchState::AuthRequired {
            return Err(TriggerValidationError::new(
                "overrides can change only before Start",
            ));
        }
        self.effective = effective_configuration(&self.revision, &overrides)?;
        self.overrides = overrides;
        let now = now.into();
        self.updated_at = now.clone();
        self.push_event(now, correlation_id, LaunchEventKind::DraftUpdated)
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
        if !matches!(self.state, LaunchState::Draft | LaunchState::AuthRequired) {
            return Err(TriggerValidationError::new(
                "the connection can change only before Start or during authentication recovery",
            ));
        }
        connection.validate_for(
            &self
                .revision
                .content
                .repository_access
                .required_scope_classes,
        )?;
        let now = now.into();
        let correlation_id = correlation_id.into();
        let recovery_target = (self.state == LaunchState::AuthRequired)
            .then(|| self.resume_state.unwrap_or(LaunchState::Draft));
        self.connection = Some(connection);
        self.blocked_reason = None;
        self.updated_at = now.clone();
        self.push_event(
            now.clone(),
            correlation_id.clone(),
            LaunchEventKind::ConnectionSelected,
        )?;
        if let Some(target) = recovery_target {
            self.transition(target, now, correlation_id)?;
            self.failure = None;
            self.resume_state = None;
        }
        Ok(())
    }

    pub fn start(
        &mut self,
        confirmation: Confirmation,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
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
        if confirmation.action != ConfirmationAction::Start
            || confirmation.subject != self.owner
            || confirmation.snapshot_digest != self.effective.digest
        {
            return Err(TriggerValidationError::field(
                "confirmation",
                "does not authorize this exact effective configuration",
            ));
        }
        let now = now.into();
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
        self.blocked_reason = Some(reason);
        self.updated_at = now.into();
        self.push_event(
            self.updated_at.clone(),
            correlation_id,
            LaunchEventKind::Blocked { reason },
        )
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
                    LaunchState::FailedRetryable,
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
        if self.state == LaunchState::PrPending
            && self.effects.iter().any(|effect| {
                matches!(
                    effect.kind,
                    LaunchEffectKind::BranchPublication | LaunchEffectKind::PullRequest
                ) && matches!(
                    effect.state,
                    EffectState::Claimed | EffectState::Reconciling | EffectState::RecoveryRequired
                )
            })
        {
            return Err(TriggerValidationError::new(
                "pull-request effects must be reconciled before cancellation",
            ));
        }
        self.confirmations.push(confirmation);
        if self.state == LaunchState::PrPending && self.pull_request.is_some() {
            self.transition(LaunchState::PrOpen, now.clone(), correlation_id.clone())?;
        }
        self.transition(LaunchState::Cancelled, now.clone(), correlation_id)?;
        self.push_event(now, "cancel", LaunchEventKind::Cancelled)
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
        if confirmation.action != ConfirmationAction::ReviewDiff
            || confirmation.subject != self.owner
            || confirmation.snapshot_digest != self.effective.digest
        {
            return Err(TriggerValidationError::field(
                "confirmation",
                "does not request a review for this exact Launch configuration",
            ));
        }
        let current = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.latest_checkpoint.as_ref())
            .map_or(0, |checkpoint| checkpoint.version);
        let requested = current.checked_add(1).ok_or_else(|| {
            TriggerValidationError::field(
                "contribution_checkpoint_version",
                "cannot advance beyond the retained checkpoint",
            )
        })?;
        let now = now.into();
        let correlation_id = correlation_id.into();
        self.contribution_checkpoint_version = Some(requested);
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

    pub fn events_after(&self, cursor: u64) -> Vec<LaunchEvent> {
        self.events
            .iter()
            .filter(|event| event.sequence > cursor)
            .cloned()
            .collect()
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
        if self.events.len() >= MAX_LAUNCH_EVENTS {
            return Err(TriggerValidationError::new(
                "Launch event retention bound reached; operator compaction is required",
            ));
        }
        let sequence = match self.events.last() {
            Some(event) => event
                .sequence
                .checked_add(1)
                .ok_or_else(|| TriggerValidationError::new("Launch event sequence is exhausted"))?,
            None => 1,
        };
        self.events.push(LaunchEvent {
            sequence,
            launch_id: self.id.clone(),
            occurred_at: occurred_at.into(),
            correlation_id: correlation_id.into(),
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
    let branch_suffix = overrides
        .branch_suffix
        .as_deref()
        .unwrap_or("work")
        .trim()
        .to_string();
    if overrides.branch_suffix.is_some() && !allowed.contains(&EditableField::BranchSuffix) {
        return Err(TriggerValidationError::field(
            "overrides.branch_suffix",
            "the branch suffix is locked by the publisher",
        ));
    }
    if branch_suffix.is_empty()
        || branch_suffix.len() > 48
        || !branch_suffix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(TriggerValidationError::field(
            "overrides.branch_suffix",
            "must contain only letters, numbers, dash, or underscore",
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
    if !overrides.optional_checks.is_empty() && !allowed.contains(&EditableField::OptionalChecks) {
        return Err(TriggerValidationError::field(
            "overrides.optional_checks",
            "optional checks are locked by the publisher",
        ));
    }
    let declared: BTreeSet<&str> = revision
        .content
        .override_policy
        .optional_checks
        .iter()
        .map(String::as_str)
        .collect();
    if overrides.optional_checks.len() > declared.len()
        || overrides
            .optional_checks
            .iter()
            .any(|check| !declared.contains(check.as_str()))
        || overrides
            .optional_checks
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != overrides.optional_checks.len()
    {
        return Err(TriggerValidationError::field(
            "overrides.optional_checks",
            "contains an undeclared or duplicate check",
        ));
    }
    let rendered_prompt = revision.content.render_prompt(&overrides.task_inputs)?;
    let digest_input = serde_json::to_vec(&(
        &revision.digest,
        &rendered_prompt,
        &branch_suffix,
        revision.content.session.permission_ceiling,
        resource_size,
        timeout_seconds,
        &overrides.optional_checks,
    ))
    .map_err(|_| TriggerValidationError::new("effective configuration cannot be encoded"))?;
    let digest = Sha256::digest(digest_input)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(EffectiveConfiguration {
        rendered_prompt,
        branch_suffix,
        permission: revision.content.session.permission_ceiling,
        resource_size,
        timeout_seconds,
        optional_checks: overrides.optional_checks.clone(),
        digest,
    })
}

fn legal_transition(from: LaunchState, to: LaunchState) -> bool {
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
