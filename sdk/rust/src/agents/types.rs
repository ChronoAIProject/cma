//! Agent resource, command, and durable read projections.

use crate::instances::{AgentStatus, Failure, SandboxStatus, WorkspaceStatus, WorktreeStatus};
use crate::profiles::{PermissionLevel, ProfileReference, SkillReference, ThinkingEffort};
use serde::{Deserialize, Serialize};

/// How this Agent was originally created.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentOrigin {
    /// Direct owner creation.
    Manual,
    /// A configured child of a Workspace Profile.
    WorkspaceProfile,
    /// A visitor Launch, never another owner's Agent.
    Trigger {
        /// Original Trigger identity.
        trigger_id: String,
        /// Original Launch identity.
        launch_id: String,
    },
}

/// Published shortcuts and creation provenance, independent of conversation history.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentSource {
    /// Owning Agent identity.
    pub agent_id: String,
    /// Immutable creation provenance.
    pub origin: AgentOrigin,
    /// Current published Trigger, when still resolvable.
    pub trigger: Option<AgentTriggerSource>,
}

/// Current published presentation of the originating Trigger.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentTriggerSource {
    /// Public visitor identity.
    pub public_id: String,
    /// Original human-readable Trigger name.
    pub title: String,
    /// Published revision used for these shortcuts.
    #[serde(deserialize_with = "crate::resources::deserialize_positive")]
    pub revision: u32,
    /// Explicit-click messages; empty for retired Triggers.
    pub quick_messages: Vec<crate::triggers::TriggerQuickMessage>,
}

string_enum! { /// Durable first-message delivery state.
    pub enum FirstMessageDelivery { Queued => "queued", DispatchFenced => "dispatch_fenced", WriteConfirmed => "write_confirmed", ExecutionObserved => "execution_observed", Completed => "completed", Failed => "failed", RecoveryRequired => "recovery_required" }
}
string_enum! { /// Owner decision for one pending Agent approval.
    pub enum ApprovalDecision { Allow => "allow", Deny => "deny", Abort => "abort" }
}
string_enum! { /// Stop reason accepted by the owner-facing command.
    pub enum StopReason { Owner => "owner" }
}

string_enum! { /// Authoritative first-message admission purpose; absent on legacy records.
    pub enum FirstMessagePurpose { Setup => "setup", Task => "task" }
}
string_enum! { /// Provenance of a provider description; absent when unknown.
    pub enum AgentDescriptionSource { ProviderTitle => "provider_title", PromptPreview => "prompt_preview" }
}

/// Durable state of the message supplied at Agent creation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FirstMessageState {
    /// Admission purpose; absent means unknown.
    pub purpose: Option<FirstMessagePurpose>,
    /// Exact durable turn, once dispatch is fenced.
    pub turn_sequence: Option<u64>,
    /// Current delivery state.
    pub state: FirstMessageDelivery,
    /// Queue timestamp.
    pub queued_at: String,
    /// Completion timestamp.
    pub completed_at: Option<String>,
    /// Failure timestamp.
    pub failed_at: Option<String>,
}

/// Public configuration granted to an Agent.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentProfileSummary {
    /// Source profile identifier.
    pub id: String,
    /// Immutable source revision.
    #[serde(deserialize_with = "crate::resources::deserialize_positive")]
    pub revision: u32,
    /// Source content digest.
    pub digest: String,
    /// Explicit model, when configured.
    pub model: Option<String>,
    /// Granted reasoning effort.
    pub thinking_effort: ThinkingEffort,
    /// Granted approval policy.
    pub permission_level: PermissionLevel,
    /// Granted skills.
    pub skills: Vec<SkillReference>,
}

/// Immutable source profile identity embedded in parent resources.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct InstanceProfileReference {
    /// Profile identifier.
    pub id: String,
    /// Immutable revision number.
    #[serde(deserialize_with = "crate::resources::deserialize_positive")]
    pub revision: u32,
    /// Immutable content digest.
    pub digest: String,
}

/// Worktree details embedded in the Agent resource.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentWorktree {
    /// Worktree identifier.
    pub id: String,
    /// Allocated name.
    pub name: String,
    /// Runtime path.
    pub path: String,
    /// Allocated Git branch.
    pub branch: String,
    /// Starting commit.
    pub base_commit: String,
    /// Allocation status.
    pub status: WorktreeStatus,
}

/// Workspace details embedded in the Agent resource.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentWorkspace {
    pub source: Option<crate::instances::WorkspaceSource>,
    /// Workspace identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Runtime path.
    pub path: String,
    /// Preparation or removal status.
    pub status: WorkspaceStatus,
    /// Immutable source profile identity.
    pub profile: InstanceProfileReference,
    /// Resolved Git commit, when applicable.
    pub resolved_commit: Option<String>,
}

/// Sandbox details embedded in the Agent resource.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentSandbox {
    /// Sandbox identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Runtime lifecycle status.
    pub status: SandboxStatus,
    /// Immutable source profile identity.
    pub profile: InstanceProfileReference,
}

/// Owner-facing Agent list item.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentSummary {
    /// Canonical runtime observations, absent on retained older runtimes.
    pub runtime_metadata: Option<AgentRuntimeMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Explicit owner rename, which takes precedence over a provider-generated title.
    pub owner_title: Option<String>,
    /// Agent identifier.
    pub id: String,
    /// Owning Sandbox identifier.
    pub sandbox_id: String,
    /// Owning Workspace identifier.
    pub workspace_id: String,
    /// Allocated Worktree identifier.
    pub worktree_id: String,
    /// Current status.
    pub status: AgentStatus,
    /// Granted profile configuration.
    pub profile: AgentProfileSummary,
    /// Human-readable title, when available.
    pub title: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
}

/// Complete owner-facing Agent resource with its hierarchy.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentBootstrap {
    /// Exact lifecycle operation that admitted this Agent and its fresh Sandbox.
    pub operation_id: String,
    /// Original owned Sandbox identifier.
    pub sandbox_id: String,
    /// Exact profile Workspace reserved for this Agent.
    pub workspace_id: String,
}

/// Complete owner-facing Agent resource with its hierarchy.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Agent {
    /// Canonical runtime observations, absent on retained older runtimes.
    pub runtime_metadata: Option<AgentRuntimeMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Explicit owner rename, which takes precedence over a provider-generated title.
    pub owner_title: Option<String>,
    /// Agent identifier.
    pub id: String,
    /// Current status.
    pub status: AgentStatus,
    /// Granted profile configuration.
    pub profile: AgentProfileSummary,
    /// Allocated Worktree.
    pub worktree: AgentWorktree,
    /// Owning Workspace.
    pub workspace: AgentWorkspace,
    /// Owning Sandbox.
    pub sandbox: AgentSandbox,
    /// Fresh lifecycle bootstrap lineage while messages can be queued before runtime readiness.
    pub bootstrap: Option<AgentBootstrap>,
    /// Public conformance identifiers.
    pub conforms_to: Vec<String>,
    /// Granted capability identifiers.
    pub reach: Vec<String>,
    /// Human-readable title.
    pub title: Option<String>,
    /// Owner-facing pause notice.
    pub paused_notice: Option<String>,
    /// Creation-message delivery state.
    pub first_message: Option<FirstMessageState>,
    /// Last safe failure.
    pub failure: Option<Failure>,
    /// Recorded terminal stop reason.
    pub stop_reason: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
    /// Readiness timestamp.
    pub ready_at: Option<String>,
    /// Stop timestamp.
    pub stopped_at: Option<String>,
    /// Deletion timestamp.
    pub deleted_at: Option<String>,
}

/// Create an Agent in an existing Workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateAgentRequest {
    /// Immutable Agent profile reference.
    pub agent_profile: ProfileReference,
    /// Optional owner title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional creation message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_message: Option<String>,
}

/// Text sent as a new message or steering input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTextRequest {
    /// Owner-provided text.
    pub text: String,
}

/// Answer one pending approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalDecisionRequest {
    /// Explicit owner decision.
    pub decision: ApprovalDecision,
}

/// Request to stop an Agent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopAgentRequest {
    /// Optional explicit owner reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<StopReason>,
}

/// Accepted input response; queued work has not necessarily executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct InputAccepted {
    /// Whether the input was placed in the durable queue.
    pub queued: bool,
}

/// Durable outcome of a transcript turn.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TranscriptOutcome {
    /// Turn has not reached a durable terminal outcome.
    Active,
    /// Turn completed successfully.
    Completed {
        /// Completion timestamp.
        at: String,
    },
    /// Turn failed.
    Failed {
        /// Failure timestamp.
        at: String,
        /// Safe failure information.
        failure: Failure,
    },
    /// Owner interrupted the turn.
    Interrupted {
        /// Interruption timestamp.
        at: String,
    },
}

string_enum! { /// Author of one chronological durable transcript message.
    pub enum TranscriptRole { User => "user", Assistant => "assistant" }
}

/// One authoritative chronological message, including owner steering within a turn.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TranscriptMessage {
    /// Positive durable log sequence.
    #[serde(deserialize_with = "crate::resources::deserialize_positive")]
    pub sequence: u64,
    /// Recorded author role.
    pub role: TranscriptRole,
    /// Exact durable message content.
    pub content: String,
    /// Durable append timestamp.
    pub appended_at: String,
}

/// One durably recorded conversation turn.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TranscriptTurn {
    /// Monotonic turn sequence.
    pub turn_sequence: u64,
    /// Start timestamp.
    pub started_at: String,
    /// Owner message.
    pub owner_message: String,
    /// Durable Agent messages.
    pub agent_messages: Vec<String>,
    /// Authoritative messages in chronological order; summaries above are not ordering evidence.
    pub messages: Vec<TranscriptMessage>,
    /// Current or terminal turn outcome.
    pub outcome: TranscriptOutcome,
}

/// Transcript folded through one durable log position.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentTranscript {
    /// Agent identifier.
    pub agent_id: String,
    /// Last included durable event sequence.
    pub through_seq: u64,
    /// Conversation turns in durable order.
    pub turns: Vec<TranscriptTurn>,
}

/// Durable usage folded through one log position.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentUsage {
    /// Latest observed turn, identified independently of context compaction.
    pub current_turn: Option<AgentTurnUsage>,
    /// Agent identifier.
    pub agent_id: String,
    /// Last included event sequence.
    pub through_seq: u64,
    /// Recorded input tokens.
    pub input_tokens: u64,
    /// Recorded output tokens.
    pub output_tokens: u64,
    /// Last recorded context usage.
    pub context_used: u64,
    /// Reported context window, when known.
    pub context_window: Option<u64>,
}

/// Runtime resources attributed to the Agent's entire Sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentResources {
    /// Agent identifier.
    pub agent_id: String,
    /// Sandbox whose counters are reported.
    pub sandbox_id: String,
    /// Runtime uptime in milliseconds.
    pub uptime_ms: u64,
    /// CPU time in microseconds, when known.
    pub cpu_usage_usec: Option<u64>,
    /// CPU allocation in millicores, when known.
    pub cpu_millis_limit: Option<u64>,
    /// Used memory in bytes, when known.
    pub memory_bytes_used: Option<u64>,
    /// Memory limit in bytes, when known.
    pub memory_bytes_limit: Option<u64>,
    /// Peak memory use in bytes, when known.
    pub memory_bytes_peak: Option<u64>,
    /// Memory stall ratio in permille, when known.
    pub stall_permille: Option<u64>,
    /// Resource ceiling events, when known.
    pub ceiling_hits: Option<u64>,
    /// Failed allocations, when known.
    pub allocation_failures: Option<u64>,
    /// Runtime process kills, when known.
    pub kills: Option<u64>,
    /// Received network bytes, when known.
    pub received_bytes: Option<u64>,
    /// Sent network bytes, when known.
    pub sent_bytes: Option<u64>,
}

/// Canonical provider conversation metadata; individual shell commands do not change it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentRuntimeMetadata {
    /// Persisted conversation working directory.
    pub working_directory: Option<String>,
    /// Provider conversation description.
    pub description: Option<String>,
    /// Explicit title or prompt preview; absent means unknown.
    pub description_source: Option<AgentDescriptionSource>,
    /// True only when exact input evidence identifies an automatic setup preview.
    pub description_is_setup_preview: Option<bool>,
}
/// Actual consumed usage within one durable Agent turn.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentTurnUsage {
    /// Durable turn sequence.
    pub turn_sequence: u64,
    /// Input tokens, including cached input.
    pub input_tokens: u64,
    /// Output tokens reported by the provider.
    pub output_tokens: u64,
}
