//! Typed durable event vocabulary.

use serde::Deserialize;
use serde_json::Value;

use crate::{Error, Result, SessionId, SessionStatus, StopReason};

/// One validated event from a session's resumable stream.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Per-session monotonic sequence and resume cursor.
    pub seq: u64,
    /// Session that owns the event.
    pub session_id: SessionId,
    /// Semantic event payload.
    pub kind: EventKind,
}

impl<'de> Deserialize<'de> for Event {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct WireEvent {
            seq: u64,
            session_id: SessionId,
            event: Value,
        }

        let wire = WireEvent::deserialize(deserializer)?;
        if wire.seq == 0 {
            return Err(serde::de::Error::custom(
                "an event sequence must be greater than zero",
            ));
        }
        let kind = parse_kind(wire.event).map_err(serde::de::Error::custom)?;
        Ok(Self {
            seq: wire.seq,
            session_id: wire.session_id,
            kind,
        })
    }
}

/// A durable semantic event emitted by CMA.
///
/// Known discriminators are validated strictly. A discriminator introduced by
/// a newer server is retained in [`EventKind::Unknown`] with its complete JSON
/// object, allowing an older client to advance its cursor without pretending it
/// understood the new event.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum EventKind {
    /// The session record was created.
    SessionCreated,
    /// Provisioning advanced to another stage.
    Progress {
        /// Current provisioning stage.
        stage: ProvisioningStage,
    },
    /// Session lifecycle state changed.
    StatusChanged {
        /// New lifecycle state.
        status: SessionStatus,
        /// Why the session stopped or paused, when known.
        stop_reason: Option<StopReason>,
        /// Human-facing failure detail, when one exists.
        detail: Option<String>,
        /// Missing session capability, when this is a permanent refusal.
        capability: Option<String>,
    },
    /// A conversation turn began.
    TurnStarted,
    /// A conversation turn completed.
    TurnCompleted,
    /// A client acquired the exclusive terminal attachment.
    TerminalAttached,
    /// A client released the exclusive terminal attachment.
    TerminalDetached,
    /// Terminal dimensions changed.
    TerminalResized {
        /// New width in columns.
        columns: u16,
        /// New height in rows.
        rows: u16,
    },
    /// A runtime opened its durable conversation.
    ConversationOpened {
        /// Opaque runtime conversation identifier.
        conversation_id: String,
    },
    /// The agent committed a complete message.
    MessageCommitted {
        /// Committed message text.
        content: String,
    },
    /// The owner sent a message.
    OwnerMessage {
        /// Owner-authored message text.
        content: String,
    },
    /// The agent started a tool call.
    ToolCallStarted {
        /// Identifier correlating later updates.
        call_id: String,
        /// Human-facing action title.
        title: String,
        /// Runtime-defined tool category.
        kind: String,
        /// Recorded and redacted arguments, when available.
        arguments: Option<ToolBody>,
    },
    /// A tool call advanced.
    ToolCallUpdated {
        /// Identifier of the call being updated.
        call_id: String,
        /// Runtime-defined status.
        status: String,
        /// Recorded and redacted result, when available.
        result: Option<ToolBody>,
    },
    /// The runtime reported cumulative token usage.
    UsageReported {
        /// Tokens sent to the model.
        input_tokens: u64,
        /// Tokens produced by the model.
        output_tokens: u64,
        /// Tokens currently occupying the context.
        context_used: u64,
        /// Context ceiling, when the runtime states one.
        context_window: Option<u64>,
    },
    /// The conversation was compacted to reclaim context.
    ContextCompacted {
        /// Previous context occupancy.
        was: u64,
        /// New context occupancy.
        now: u64,
        /// Context ceiling, when known.
        window: Option<u64>,
    },
    /// The runtime changed its permission mode.
    ModeChanged {
        /// Runtime-defined mode name.
        mode: String,
    },
    /// The agent replaced its current plan.
    PlanSet {
        /// Complete ordered plan.
        entries: Vec<PlanStep>,
    },
    /// The runtime announced settable configuration.
    ConfigOptionsSet {
        /// Complete configuration option set.
        options: Vec<ConfigOption>,
    },
    /// The runtime announced invocable commands.
    AvailableCommandsSet {
        /// Complete command set.
        commands: Vec<AvailableCommand>,
    },
    /// Part of the live conversation was lost before it became durable.
    ConversationGap {
        /// Number of dropped frames.
        frames: u64,
    },
    /// The owner must complete a vendor sign-in.
    SignInRequired {
        /// Page the owner opens on their own device.
        url: String,
        /// One-time code, when supplied by the runtime.
        code: Option<String>,
        /// Runtime-authored guidance, when supplied.
        message: Option<String>,
    },
    /// The runtime is blocked on an owner decision.
    ApprovalRequested {
        /// Identifier used to resolve the request.
        request_id: String,
        /// Human-facing action, when supplied.
        action: Option<String>,
        /// Additional human-facing detail, when supplied.
        detail: Option<String>,
    },
    /// A pending approval was resolved.
    ApprovalResolved {
        /// Identifier of the resolved request.
        request_id: String,
        /// Whether the requested action was permitted.
        allowed: bool,
    },
    /// A session moved between interaction planes.
    PlaneChanged {
        /// Previous plane.
        from: SessionPlane,
        /// New plane.
        to: SessionPlane,
        /// Snapshot moment used for the move, in RFC 3339 form.
        snapshot_at: String,
    },
    /// Sustained memory pressure began.
    MemoryPressureOpened {
        /// Bytes in use when the episode opened.
        used_bytes: u64,
        /// Memory limit used for the reading.
        limit_bytes: u64,
        /// Occupancy as parts per thousand.
        occupancy_permille: u16,
    },
    /// Sustained memory pressure ended.
    MemoryPressureClosed {
        /// Peak bytes held during the episode.
        peak_bytes: u64,
        /// Memory limit used for the readings.
        limit_bytes: u64,
        /// Peak occupancy as parts per thousand.
        peak_permille: u16,
        /// Episode length from a monotonic clock.
        duration_seconds: u32,
    },
    /// The service established that a runtime disappeared.
    RuntimeLost {
        /// Opaque provider runtime identifier.
        runtime_id: String,
        /// Provider-derived cause, when one could be established.
        cause: Option<RuntimeLossCause>,
    },
    /// An event discriminator this SDK version does not know.
    Unknown {
        /// Unrecognised value of the event's `type` field.
        event_type: String,
        /// Complete event object, including its discriminator.
        payload: Value,
    },
}

/// A step on the way to a usable runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvisioningStage {
    /// The request was accepted and the session exists.
    Accepted,
    /// A runtime was requested.
    RuntimeRequested,
    /// The provider created the runtime.
    RuntimeCreated,
    /// Granted skills were installed.
    SkillsInstalled,
    /// The service is connecting to the runtime.
    Attaching,
    /// The session became usable.
    Ready,
}

/// Why a session runtime disappeared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeLossCause {
    /// The runtime exceeded its memory limit.
    OutOfMemory,
    /// The runtime exited successfully.
    Ended,
    /// The runtime exited with a failure.
    Failed,
    /// The machine hosting the runtime disappeared.
    Displaced,
    /// The provider supplied a reason this vocabulary cannot read.
    Unrecognised,
}

/// Which interaction plane drives a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPlane {
    /// Human-facing terminal driven by bytes.
    Terminal,
    /// Structured conversation driven by messages.
    Conversation,
}

/// A bounded, redacted tool request or result.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ToolBody {
    /// Body text after server-side redaction.
    pub text: String,
    /// Whether the server truncated the original body.
    #[serde(default)]
    pub truncated: bool,
}

/// One runtime-authored plan step.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PlanStep {
    /// Human-facing step title.
    pub title: String,
    /// Runtime-defined step status.
    pub status: String,
}

/// One setting the runtime says can be changed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ConfigOption {
    /// Runtime-defined option identifier.
    pub id: String,
    /// Human-facing option name.
    pub name: String,
    /// Runtime-defined value kind.
    pub kind: String,
}

/// One action the runtime says can be invoked.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AvailableCommand {
    /// Runtime-defined command name.
    pub name: String,
    /// Human-facing command description.
    pub description: String,
}

const KNOWN_EVENT_TYPES: &[&str] = &[
    "session_created",
    "progress",
    "status_changed",
    "turn_started",
    "turn_completed",
    "terminal_attached",
    "terminal_detached",
    "terminal_resized",
    "conversation_opened",
    "message_committed",
    "owner_message",
    "tool_call_started",
    "tool_call_updated",
    "usage_reported",
    "context_compacted",
    "mode_changed",
    "plan_set",
    "config_options_set",
    "available_commands_set",
    "conversation_gap",
    "sign_in_required",
    "approval_requested",
    "approval_resolved",
    "plane_changed",
    "memory_pressure_opened",
    "memory_pressure_closed",
    "runtime_lost",
];

fn parse_kind(value: Value) -> Result<EventKind> {
    let event_type = value
        .as_object()
        .ok_or_else(|| Error::protocol("an event payload must be a JSON object"))?
        .get("type")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| Error::protocol("an event payload requires a non-empty string 'type'"))?
        .to_owned();

    if !KNOWN_EVENT_TYPES.contains(&event_type.as_str()) {
        return Ok(EventKind::Unknown {
            event_type,
            payload: value,
        });
    }

    serde_json::from_value::<KnownEventKind>(value)
        .map(EventKind::from)
        .map_err(|error| {
            Error::protocol(format!(
                "known event '{event_type}' does not match its wire shape: {error}"
            ))
        })
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
enum KnownEventKind {
    SessionCreated,
    Progress {
        stage: ProvisioningStage,
    },
    StatusChanged {
        status: SessionStatus,
        #[serde(default)]
        stop_reason: Option<StopReason>,
        #[serde(default)]
        detail: Option<String>,
        #[serde(default)]
        capability: Option<String>,
    },
    TurnStarted,
    TurnCompleted,
    TerminalAttached,
    TerminalDetached,
    TerminalResized {
        columns: u16,
        rows: u16,
    },
    ConversationOpened {
        conversation_id: String,
    },
    MessageCommitted {
        content: String,
    },
    OwnerMessage {
        content: String,
    },
    ToolCallStarted {
        call_id: String,
        title: String,
        kind: String,
        #[serde(default)]
        arguments: Option<ToolBody>,
    },
    ToolCallUpdated {
        call_id: String,
        status: String,
        #[serde(default)]
        result: Option<ToolBody>,
    },
    UsageReported {
        input_tokens: u64,
        output_tokens: u64,
        context_used: u64,
        #[serde(default)]
        context_window: Option<u64>,
    },
    ContextCompacted {
        was: u64,
        now: u64,
        #[serde(default)]
        window: Option<u64>,
    },
    ModeChanged {
        mode: String,
    },
    PlanSet {
        entries: Vec<PlanStep>,
    },
    ConfigOptionsSet {
        options: Vec<ConfigOption>,
    },
    AvailableCommandsSet {
        commands: Vec<AvailableCommand>,
    },
    ConversationGap {
        frames: u64,
    },
    SignInRequired {
        url: String,
        #[serde(default)]
        code: Option<String>,
        #[serde(default)]
        message: Option<String>,
    },
    ApprovalRequested {
        request_id: String,
        #[serde(default)]
        action: Option<String>,
        #[serde(default)]
        detail: Option<String>,
    },
    ApprovalResolved {
        request_id: String,
        allowed: bool,
    },
    PlaneChanged {
        from: SessionPlane,
        to: SessionPlane,
        snapshot_at: String,
    },
    MemoryPressureOpened {
        used_bytes: u64,
        limit_bytes: u64,
        occupancy_permille: u16,
    },
    MemoryPressureClosed {
        peak_bytes: u64,
        limit_bytes: u64,
        peak_permille: u16,
        duration_seconds: u32,
    },
    RuntimeLost {
        runtime_id: String,
        #[serde(default)]
        cause: Option<RuntimeLossCause>,
    },
}

impl From<KnownEventKind> for EventKind {
    fn from(value: KnownEventKind) -> Self {
        match value {
            KnownEventKind::SessionCreated => Self::SessionCreated,
            KnownEventKind::Progress { stage } => Self::Progress { stage },
            KnownEventKind::StatusChanged {
                status,
                stop_reason,
                detail,
                capability,
            } => Self::StatusChanged {
                status,
                stop_reason,
                detail,
                capability,
            },
            KnownEventKind::TurnStarted => Self::TurnStarted,
            KnownEventKind::TurnCompleted => Self::TurnCompleted,
            KnownEventKind::TerminalAttached => Self::TerminalAttached,
            KnownEventKind::TerminalDetached => Self::TerminalDetached,
            KnownEventKind::TerminalResized { columns, rows } => {
                Self::TerminalResized { columns, rows }
            }
            KnownEventKind::ConversationOpened { conversation_id } => {
                Self::ConversationOpened { conversation_id }
            }
            KnownEventKind::MessageCommitted { content } => Self::MessageCommitted { content },
            KnownEventKind::OwnerMessage { content } => Self::OwnerMessage { content },
            KnownEventKind::ToolCallStarted {
                call_id,
                title,
                kind,
                arguments,
            } => Self::ToolCallStarted {
                call_id,
                title,
                kind,
                arguments,
            },
            KnownEventKind::ToolCallUpdated {
                call_id,
                status,
                result,
            } => Self::ToolCallUpdated {
                call_id,
                status,
                result,
            },
            KnownEventKind::UsageReported {
                input_tokens,
                output_tokens,
                context_used,
                context_window,
            } => Self::UsageReported {
                input_tokens,
                output_tokens,
                context_used,
                context_window,
            },
            KnownEventKind::ContextCompacted { was, now, window } => {
                Self::ContextCompacted { was, now, window }
            }
            KnownEventKind::ModeChanged { mode } => Self::ModeChanged { mode },
            KnownEventKind::PlanSet { entries } => Self::PlanSet { entries },
            KnownEventKind::ConfigOptionsSet { options } => Self::ConfigOptionsSet { options },
            KnownEventKind::AvailableCommandsSet { commands } => {
                Self::AvailableCommandsSet { commands }
            }
            KnownEventKind::ConversationGap { frames } => Self::ConversationGap { frames },
            KnownEventKind::SignInRequired { url, code, message } => {
                Self::SignInRequired { url, code, message }
            }
            KnownEventKind::ApprovalRequested {
                request_id,
                action,
                detail,
            } => Self::ApprovalRequested {
                request_id,
                action,
                detail,
            },
            KnownEventKind::ApprovalResolved {
                request_id,
                allowed,
            } => Self::ApprovalResolved {
                request_id,
                allowed,
            },
            KnownEventKind::PlaneChanged {
                from,
                to,
                snapshot_at,
            } => Self::PlaneChanged {
                from,
                to,
                snapshot_at,
            },
            KnownEventKind::MemoryPressureOpened {
                used_bytes,
                limit_bytes,
                occupancy_permille,
            } => Self::MemoryPressureOpened {
                used_bytes,
                limit_bytes,
                occupancy_permille,
            },
            KnownEventKind::MemoryPressureClosed {
                peak_bytes,
                limit_bytes,
                peak_permille,
                duration_seconds,
            } => Self::MemoryPressureClosed {
                peak_bytes,
                limit_bytes,
                peak_permille,
                duration_seconds,
            },
            KnownEventKind::RuntimeLost { runtime_id, cause } => {
                Self::RuntimeLost { runtime_id, cause }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_events_retain_the_complete_payload() {
        let raw = serde_json::json!({
            "seq": 8,
            "session_id": "sesn_fixture",
            "event": { "type": "future_event", "answer": 42 }
        });
        let event: Event = serde_json::from_value(raw.clone()).expect("unknown variants are valid");
        let EventKind::Unknown {
            event_type,
            payload,
        } = event.kind
        else {
            panic!("the event should be carried as unknown");
        };
        assert_eq!(event_type, "future_event");
        assert_eq!(payload, raw["event"]);
    }

    #[test]
    fn malformed_known_events_fail_loudly() {
        let error = serde_json::from_value::<Event>(serde_json::json!({
            "seq": 1,
            "session_id": "sesn_fixture",
            "event": { "type": "terminal_resized", "columns": "wide", "rows": 40 }
        }))
        .expect_err("a known event with a wrong field type must fail");
        assert!(error.to_string().contains("terminal_resized"));
    }

    #[test]
    fn an_event_requires_a_real_cursor_and_discriminator() {
        for raw in [
            serde_json::json!({
                "seq": 0,
                "session_id": "sesn_fixture",
                "event": { "type": "turn_started" }
            }),
            serde_json::json!({
                "seq": 1,
                "session_id": "sesn_fixture",
                "event": { "detail": "missing type" }
            }),
        ] {
            assert!(serde_json::from_value::<Event>(raw).is_err());
        }
    }

    #[test]
    fn every_known_event_discriminator_deserializes_through_its_typed_arm() {
        let payloads = [
            serde_json::json!({ "type": "session_created" }),
            serde_json::json!({ "type": "progress", "stage": "runtime_requested" }),
            serde_json::json!({
                "type": "status_changed",
                "status": "idle",
                "stop_reason": "requires_action",
                "detail": "waiting",
                "capability": "conversation"
            }),
            serde_json::json!({ "type": "turn_started" }),
            serde_json::json!({ "type": "turn_completed" }),
            serde_json::json!({ "type": "terminal_attached" }),
            serde_json::json!({ "type": "terminal_detached" }),
            serde_json::json!({
                "type": "terminal_resized", "columns": 120, "rows": 40
            }),
            serde_json::json!({
                "type": "conversation_opened", "conversation_id": "conv_fixture"
            }),
            serde_json::json!({ "type": "message_committed", "content": "done" }),
            serde_json::json!({ "type": "owner_message", "content": "start" }),
            serde_json::json!({
                "type": "tool_call_started",
                "call_id": "call_fixture",
                "title": "Read file",
                "kind": "read",
                "arguments": { "text": "README.md", "truncated": false }
            }),
            serde_json::json!({
                "type": "tool_call_updated",
                "call_id": "call_fixture",
                "status": "completed",
                "result": { "text": "contents", "truncated": true }
            }),
            serde_json::json!({
                "type": "usage_reported",
                "input_tokens": 10,
                "output_tokens": 2,
                "context_used": 12,
                "context_window": 200000
            }),
            serde_json::json!({
                "type": "context_compacted", "was": 100, "now": 40, "window": 200
            }),
            serde_json::json!({ "type": "mode_changed", "mode": "guarded" }),
            serde_json::json!({
                "type": "plan_set",
                "entries": [{ "title": "Test", "status": "pending" }]
            }),
            serde_json::json!({
                "type": "config_options_set",
                "options": [{ "id": "model", "name": "Model", "kind": "select" }]
            }),
            serde_json::json!({
                "type": "available_commands_set",
                "commands": [{ "name": "review", "description": "Review work" }]
            }),
            serde_json::json!({ "type": "conversation_gap", "frames": 3 }),
            serde_json::json!({
                "type": "sign_in_required",
                "url": "https://login.example.test",
                "code": "ABCD",
                "message": "Sign in"
            }),
            serde_json::json!({
                "type": "approval_requested",
                "request_id": "req_fixture",
                "action": "write",
                "detail": "writes a file"
            }),
            serde_json::json!({
                "type": "approval_resolved", "request_id": "req_fixture", "allowed": true
            }),
            serde_json::json!({
                "type": "plane_changed",
                "from": "terminal",
                "to": "conversation",
                "snapshot_at": "2026-08-24T08:00:00Z"
            }),
            serde_json::json!({
                "type": "memory_pressure_opened",
                "used_bytes": 90,
                "limit_bytes": 100,
                "occupancy_permille": 900
            }),
            serde_json::json!({
                "type": "memory_pressure_closed",
                "peak_bytes": 95,
                "limit_bytes": 100,
                "peak_permille": 950,
                "duration_seconds": 30
            }),
            serde_json::json!({
                "type": "runtime_lost",
                "runtime_id": "runtime_fixture",
                "cause": "out_of_memory"
            }),
        ];

        for (index, payload) in payloads.into_iter().enumerate() {
            let event: Event = serde_json::from_value(serde_json::json!({
                "seq": index + 1,
                "session_id": "sesn_fixture",
                "event": payload
            }))
            .expect("every server-owned discriminator has a typed shape");
            assert!(!matches!(event.kind, EventKind::Unknown { .. }));
        }
    }
}
