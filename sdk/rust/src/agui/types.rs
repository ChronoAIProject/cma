//! AG-UI requests and conversation event envelopes.

use crate::{
    agents::{ApprovalDecision, FirstMessageDelivery},
    instances::{SandboxStatus, WorkspaceStatus, WorktreeStatus},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

string_enum! { /// Emitted AG-UI event type.
    pub enum EventKind {
        RunStarted => "RUN_STARTED", RunFinished => "RUN_FINISHED", RunError => "RUN_ERROR",
        TextMessageStart => "TEXT_MESSAGE_START", TextMessageContent => "TEXT_MESSAGE_CONTENT", TextMessageEnd => "TEXT_MESSAGE_END",
        ToolCallStart => "TOOL_CALL_START", ToolCallEnd => "TOOL_CALL_END", StateDelta => "STATE_DELTA",
        ReasoningMessageContent => "REASONING_MESSAGE_CONTENT"
    }
}
string_enum! { /// Message role supported for a new Agent run.
    pub enum UserRole { User => "user" }
}
string_enum! { /// Explicit approval resolution state.
    pub enum ResumeStatus { Resolved => "resolved" }
}

/// One explicit user message for an existing Agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMessage {
    /// The supported input role.
    pub role: UserRole,
    /// User-authored text.
    pub content: String,
}

/// Explicit decision carried by an approval answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumePayload {
    /// Owner decision.
    pub decision: ApprovalDecision,
}

/// One explicit approval answer in the AG-UI protocol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resume {
    /// Server-issued pending interrupt identity.
    pub interrupt_id: String,
    /// Explicit resolved state.
    pub status: ResumeStatus,
    /// Owner approval decision.
    pub payload: ResumePayload,
}

/// Admit a message or approval answer against an existing frozen Agent profile.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunRequest {
    #[doc = "Optional conditional-send fields; cannot override the Agent profile."]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forwarded_props: Option<ForwardedProps>,
    /// Existing Agent identity.
    pub thread_id: String,
    /// Optional caller run correlation identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Explicit user input, omitted for approval-only runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<UserMessage>,
    /// At most one explicit approval answer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resume: Vec<Resume>,
}

#[doc = "Supported AG-UI conditional-send extension."]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ForwardedProps {
    #[doc = "The final answer required for admitting this message."]
    pub answer_context: crate::answer_context::AnswerContext,
}

/// One typed AG-UI event envelope; payload preserves named protocol extensions.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEvent {
    /// Emitted AG-UI type.
    #[serde(rename = "type")]
    pub kind: EventKind,
    /// Existing Agent identity.
    pub thread_id: String,
    /// Durable turn identity.
    pub run_id: String,
    /// Durable event timestamp, absent for live-only frames.
    pub timestamp: Option<String>,
    /// Whether this frame is explicitly nondurable live output.
    #[serde(default, rename = "cma:live")]
    pub live: bool,
    /// Cursor acknowledging a complete durable record, never inherited onto live frames.
    #[serde(skip)]
    pub cursor: Option<u64>,
    /// Type-specific protocol fields, including state patches and interrupt payloads.
    #[serde(flatten)]
    pub payload: BTreeMap<String, serde_json::Value>,
}

/// Public protocol and feature declaration, preserving additive feature names.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// Declared protocol identity.
    pub protocol: String,
    /// Supported transport names.
    pub transports: Vec<String>,
    /// Supported and explicitly unsupported named events.
    pub events: BTreeMap<String, bool>,
    /// Grouped protocol and CMA-specific feature declarations.
    pub features: BTreeMap<String, serde_json::Value>,
    /// Conformance class identifiers.
    pub conforms_to: Vec<String>,
}

/// Read-only hierarchy companion for one Agent.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxCompanion {
    /// Agent identity.
    pub thread_id: String,
    /// Sandbox snapshot with lifecycle notices and rotation metadata.
    pub sandbox: SandboxSnapshot,
    /// Workspace identity and preparation state.
    pub workspace: WorkspaceSnapshot,
    /// Worktree identity and allocation state.
    pub worktree: WorktreeSnapshot,
    /// Optional first-message delivery state.
    pub first_message: Option<FirstMessageSnapshot>,
}

/// Sandbox lifecycle fields exposed by the read-only AG-UI companion.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxSnapshot {
    /// Owning Sandbox identity.
    pub id: String,
    /// Owner-facing name.
    pub name: String,
    /// Closed durable lifecycle status.
    pub status: SandboxStatus,
    /// Whether the Sandbox is pausing or paused.
    pub paused: bool,
    /// Server-composed pause notice; preserve its wording.
    pub paused_notice: Option<String>,
    /// Completed image rotation accounting.
    pub rotation: RotationSnapshot,
}

/// Rotation accounting retains the backend's snake-case nested representation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RotationSnapshot {
    /// Number of completed rotations.
    pub rotations: u32,
    /// Last completed checkpoint boundary, absent before the first rotation.
    pub rotated_at: Option<String>,
}

/// Workspace preparation snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspaceSnapshot {
    /// Owning Workspace identity.
    pub id: String,
    /// Owner-facing name.
    pub name: String,
    /// Absolute workspace path.
    pub path: String,
    /// Closed preparation or removal status.
    pub status: WorkspaceStatus,
}

/// Agent Worktree allocation snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorktreeSnapshot {
    /// Allocated Worktree identity.
    pub id: String,
    /// Worktree name.
    pub name: String,
    /// Absolute worktree path.
    pub path: String,
    /// Closed allocation or removal status.
    pub status: WorktreeStatus,
}

/// Durable first-message delivery snapshot, never evidence of automatic admission.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirstMessageSnapshot {
    /// Closed delivery status.
    pub state: FirstMessageDelivery,
    /// Recorded delivery attempt.
    pub attempt: u32,
    /// Original queue timestamp.
    pub queued_at: String,
    /// Completion timestamp when durable positive evidence exists.
    pub completed_at: Option<String>,
    /// Failure timestamp when durable negative evidence exists.
    pub failed_at: Option<String>,
}
