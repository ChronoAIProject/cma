use mongodb::bson::DateTime;
use serde::{Deserialize, Serialize};

/// An existing, qualified definition. Selection never changes its identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleaseTarget {
    pub image: String,
    pub plan_sha256: String,
    pub target_id: String,
    pub target_revision: i64,
    pub pool_name: String,
    pub pool_uid: String,
    pub template_name: String,
    pub template_uid: String,
    pub qualification_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleasePair {
    pub agent: ReleaseTarget,
    pub machine: Option<ReleaseTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WriterIdentity {
    pub deployment: String,
    pub instance_id: String,
    pub config_sha256: String,
    pub epoch: i64,
    pub fence: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Active,
    Freezing,
    Frozen,
    Prepared,
    CommittedVerifying,
    AbortedHold,
    FailedHold,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleaseHead {
    #[serde(rename = "_id")]
    pub deployment: String,
    pub schema_version: i64,
    pub epoch: i64,
    pub control_revision: i64,
    pub admission_sequence: i64,
    pub active_effects: i64,
    pub phase: Phase,
    pub pair: ReleasePair,
    pub writer: Option<WriterIdentity>,
    pub writer_fence: i64,
    pub operation_id: Option<String>,
    pub protected_pairs: Vec<ReleasePair>,
    /// Set by Mongo $currentDate; never supplied as release-order authority.
    pub committed_at: DateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorIdentity {
    pub subject: String,
    /// Hash of the authenticated exact execution authorization, not a bearer.
    pub authorization_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BootstrapRequest {
    pub deployment: String,
    pub operation_id: String,
    pub operator: OperatorIdentity,
    pub pair: ReleasePair,
    pub fallback_pair: ReleasePair,
    pub writer: WriterIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TransitionRequest {
    pub deployment: String,
    pub operation_id: String,
    pub operator: OperatorIdentity,
    pub expected_epoch: i64,
    pub expected_control_revision: i64,
    pub before: ReleasePair,
    pub after: ReleasePair,
    pub next_writer: WriterIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "request", rename_all = "snake_case")]
pub(crate) enum OperationRequest {
    Bootstrap(Box<BootstrapRequest>),
    Transition(Box<TransitionRequest>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AdvanceAction {
    ConfirmDrained,
    Prepare,
    Commit,
    Activate,
    Hold,
    Abort,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdvanceRequest {
    pub deployment: String,
    pub operation_id: String,
    pub operator: OperatorIdentity,
    pub expected_epoch: i64,
    pub expected_control_revision: i64,
    pub expected_phase: Phase,
    pub action: AdvanceAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ControlReceipt {
    pub request_sha256: String,
    pub evidence_sha256: String,
    pub epoch: i64,
    pub control_revision: i64,
    pub phase: Phase,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleaseOperation {
    #[serde(rename = "_id")]
    pub key: String,
    pub schema_version: i64,
    pub deployment: String,
    pub operation_id: String,
    pub request_sha256: String,
    pub request: OperationRequest,
    /// A precommit reversal abort returns to this held operation, never Active.
    pub predecessor_operation_id: Option<String>,
    pub receipts: Vec<ControlReceipt>,
    /// Single terminal exit when the immutable ordinary audit is already full.
    #[serde(default)]
    pub terminal_recovery_receipt: Option<ControlReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EffectIntent {
    pub effect_id: String,
    pub resource_identity_sha256: String,
    pub request_sha256: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObservedOutcome {
    pub receipt_sha256: String,
    pub receiver_incarnation_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EffectPermit {
    #[serde(rename = "_id")]
    pub key: String,
    pub schema_version: i64,
    pub writer: WriterIdentity,
    pub intent: EffectIntent,
    pub admitted_sequence: i64,
    pub revision: i64,
    /// Provider acknowledgement is observation, not logical finality.
    pub observations: Vec<ObservedOutcome>,
    pub terminal_evidence_sha256: Option<String>,
}

/// Reconciliation of an existing permit never grants another dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EffectAdmission {
    New(EffectPermit),
    Existing(EffectPermit),
}
