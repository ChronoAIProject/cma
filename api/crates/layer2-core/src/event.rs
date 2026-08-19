//! The durable event log, and the ephemeral stream that rides alongside it.
//!
//! The split matters. Durable events are semantic and carry a per-session
//! monotonic sequence, so a client can resume from a cursor. Ephemeral frames
//! are liveness deltas — terminal output, token-level chunks — and are never
//! persisted and never numbered. Giving a live frame a sequence number would
//! punch permanent holes in the durable sequence that a resuming client could
//! not distinguish from data loss.

use serde::{Deserialize, Serialize};

use crate::ids::SessionId;
use crate::session::{SessionStatus, StopReason};

/// A durable, ordered event on a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Session this event belongs to.
    pub session_id: SessionId,
    /// Monotonic sequence, per session. Never global.
    pub seq: u64,
    /// Event discriminator.
    pub kind: EventKind,
    /// When this event was appended to the durable log, RFC 3339 in UTC.
    ///
    /// Minted by the log when the event is written, not by whoever noticed
    /// it: the moment a client should be shown is the moment it became
    /// history, and that is the same for every path that writes. Empty for an
    /// event read from a control plane that predates this field — a rolling
    /// deployment tolerates it and a client renders no time rather than an
    /// invented one.
    #[serde(default)]
    pub appended_at: String,
}

/// An event decoded from durable storage before it is projected to callers.
///
/// Writers only accept [`Event`], so the unknown arm cannot be written on
/// purpose. It exists on the read side so a control plane can cross a rolling
/// deployment with a newer event vocabulary without treating that vocabulary
/// as corrupt storage.
#[derive(Debug, Clone)]
pub struct StoredEvent {
    /// Session this event belongs to.
    pub session_id: SessionId,
    /// Monotonic sequence as it appears in storage.
    pub seq: u64,
    /// When this event was appended, as it was stored. Empty when a writer
    /// predating the field produced the document.
    pub appended_at: String,
    /// A kind understood by this build, or the future value it cannot project.
    pub kind: StoredEventKind,
}

impl StoredEvent {
    /// Return the public event when this build understands its kind.
    pub fn into_known(self) -> Option<Event> {
        match self.kind {
            StoredEventKind::Known(kind) => Some(Event {
                session_id: self.session_id,
                seq: self.seq,
                kind,
                appended_at: self.appended_at,
            }),
            StoredEventKind::Unknown { .. } => None,
        }
    }
}

impl From<Event> for StoredEvent {
    fn from(event: Event) -> Self {
        Self {
            session_id: event.session_id,
            seq: event.seq,
            appended_at: event.appended_at,
            kind: StoredEventKind::Known(event.kind),
        }
    }
}

/// The read-side durable vocabulary.
#[derive(Debug, Clone)]
pub enum StoredEventKind {
    /// A kind this build can validate and project.
    Known(EventKind),
    /// A kind written by a newer build.
    Unknown {
        /// Unrecognized `type` discriminator exactly as it was stored.
        tag: String,
        /// Remaining fields, in whatever shape they can be rendered for a
        /// person diagnosing the gap. Nothing reads them.
        payload: serde_json::Value,
    },
}

/// The semantic event vocabulary. Superset of what any single interface projects.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum EventKind {
    /// The session was created.
    SessionCreated,
    /// A step on the way to a usable session.
    ///
    /// Starting a session is slow enough that "wait" is not an answer on its
    /// own — a person watching a silent screen cannot tell a slow start from a
    /// stuck one. These say which part is taking the time.
    Progress {
        stage: crate::session::ProvisioningStage,
    },
    /// Session status changed.
    StatusChanged {
        /// The new status.
        status: SessionStatus,
        /// Why, when the change was a stop or a pause.
        #[serde(skip_serializing_if = "Option::is_none")]
        stop_reason: Option<StopReason>,
        /// What went wrong, when something did.
        ///
        /// Present only for a failure, and written for the person who has to
        /// act on it. Without this a session that could not start is
        /// indistinguishable from one that ended normally, which is precisely
        /// the moment an explanation is worth most.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        /// The ability this session lacked, when that is what stopped it.
        ///
        /// Named, beside the prose rather than inside it. A refusal is
        /// arrived at typed — the core knows *which* ability was missing —
        /// and flattening it into a sentence throws away the only part a
        /// caller could act on. `API_Overview.md` rule 4 asks errors to name
        /// what is missing, and a sentence containing the word does not
        /// satisfy that: nothing downstream can read a sentence.
        ///
        /// Absent for every failure that was not a refusal, which is most of
        /// them. A provider that was merely unreachable lacks nothing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capability: Option<String>,
    },
    /// A turn began.
    TurnStarted,
    /// A turn completed.
    TurnCompleted,
    /// A client took the exclusive terminal attachment.
    TerminalAttached,
    /// The exclusive terminal attachment was released.
    TerminalDetached,
    /// The terminal was resized.
    TerminalResized {
        /// New width in columns.
        columns: u16,
        /// New height in rows.
        rows: u16,
    },
    /// The runtime named the conversation this session is holding.
    ///
    /// Recorded rather than remembered, for the reason §5 gives for recording
    /// anything: a control plane that restarts holds no state that matters, and
    /// what it needs to take a live runtime back over is what that runtime
    /// calls the conversation already running in it. Without this the
    /// replacement opens a second conversation beside the first — the agent
    /// loses its context, and the owner, reading a log that has everything,
    /// cannot tell.
    ConversationOpened {
        /// The runtime's own name for it, carried through opaquely.
        conversation_id: String,
    },
    /// A message the agent finished and committed to.
    ///
    /// Durable because it is what the agent actually said; the token-level
    /// pieces it was assembled from are liveness and travel as ephemeral
    /// frames. Numbering every delta would punch permanent holes in the
    /// sequence a resuming client cursors on, and replaying them would show
    /// somebody their conversation being typed a second time.
    MessageCommitted {
        /// What the agent said, as it committed it.
        content: String,
    },
    /// The owner sent a message to the agent.
    ///
    /// Durable for the same reason the agent's committed message is: the
    /// conversation a client replays must show both sides, and the owner's
    /// turn is the one the surface labels with the time it was sent. Recorded
    /// here rather than reconstructed from the runtime's transcript, so the
    /// word this service keeps for it stays its own and no kernel's vocabulary
    /// has to be understood to know what the owner said.
    OwnerMessage {
        /// What the owner sent, verbatim.
        content: String,
    },
    /// The agent began using a tool.
    ToolCallStarted {
        /// Correlates with every later update to the same call.
        call_id: String,
        /// What it is doing, as the runtime would show a person.
        title: String,
        /// The kind of thing it is, in the runtime's own words.
        ///
        /// Opaque here. Interfaces render it and the core never branches on
        /// it: the set differs between runtimes and grows without warning, and
        /// a session must not fail because a kernel invented a tool category.
        kind: String,
    },
    /// A tool call moved on.
    ToolCallUpdated {
        /// The call this is about.
        call_id: String,
        /// Where it has got to, in the runtime's own words.
        status: String,
    },
    /// What this session has used, as its runtime reports it.
    ///
    /// Totals rather than deltas, because that is what runtimes report and
    /// because a lost delta would silently understate a bill forever, where a
    /// lost total is corrected by the next one.
    UsageReported {
        /// Tokens sent to the model.
        input_tokens: u64,
        /// Tokens the model produced.
        output_tokens: u64,
        /// How much of the model's context the conversation currently fills.
        context_used: u64,
    },
    /// Part of the conversation was lost rather than recorded.
    ///
    /// Written where the loss happened, in the same spirit as a rotation's
    /// timestamp: the owner is shown a boundary rather than left to puzzle
    /// over a conversation that skips. It means the runtime held more than it
    /// could while nothing was listening — a long control-plane restart — and
    /// dropped the oldest of it.
    ConversationGap {
        /// How many frames were dropped, as the runtime counted them.
        frames: u64,
    },
    /// The runtime is blocked awaiting a decision from the client.
    ApprovalRequested {
        /// Correlates with the resolving decision.
        request_id: String,
        /// What the runtime is asking to do, as it would show a person.
        ///
        /// Absent on approvals recorded before this was carried, and on a
        /// runtime that offers no description of what it is asking.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<String>,
        /// Anything further the runtime offered about the request.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// The session was moved onto the other plane, because its owner asked.
    ///
    /// Durable because the surface changed under somebody, and the event log
    /// is where they read their session's history. It carries both planes and
    /// the moment the conversation was captured: anything the agent did after
    /// that moment did not travel, and a boundary somebody can see is one they
    /// can recognise.
    PlaneChanged {
        /// The plane the session was on.
        from: crate::options::SessionMode,
        /// The plane it is on now.
        to: crate::options::SessionMode,
        /// When its conversation was taken, RFC 3339.
        snapshot_at: String,
    },
    /// A pending approval was resolved.
    ApprovalResolved {
        /// The request that was resolved.
        request_id: String,
        /// Whether the action was permitted.
        allowed: bool,
    },
}

/// Every discriminator this build owns, exactly as serde spells it.
///
/// Must name every variant of [`EventKind`]. A variant missing from here would
/// be read back as somebody else's vocabulary and skipped rather than
/// projected, which is why the test module walks the enum against this list.
const KNOWN_TAGS: &[&str] = &[
    "session_created",
    "progress",
    "status_changed",
    "turn_started",
    "turn_completed",
    "conversation_opened",
    "message_committed",
    "owner_message",
    "tool_call_started",
    "tool_call_updated",
    "usage_reported",
    "conversation_gap",
    "terminal_attached",
    "terminal_detached",
    "terminal_resized",
    "approval_requested",
    "approval_resolved",
    "plane_changed",
];

impl EventKind {
    /// Whether `tag` names a variant this build must validate strictly.
    ///
    /// Storage asks this before deserializing, so it can tell a discriminator
    /// from a newer writer apart from a payload that is broken under a
    /// discriminator we own. The first is history this build cannot read; the
    /// second is corruption, and reporting them the same way would either
    /// hide real damage or make a rolling deployment look like one.
    pub fn recognizes_tag(tag: &str) -> bool {
        KNOWN_TAGS.contains(&tag)
    }
}

/// A liveness delta. Deliberately carries no sequence number: it is not part of
/// the durable log and must never be replayed from one.
#[derive(Debug, Clone)]
pub enum LiveFrame {
    /// Raw output from the runtime's terminal.
    TerminalOutput(Vec<u8>),
    /// A piece of a message the agent is still assembling.
    ///
    /// The conversation plane's counterpart to terminal output, and ephemeral
    /// for the same reason: it is what is happening, not what happened. The
    /// finished message is committed durably, and a client that missed the
    /// pieces finds the whole thing waiting when it reconnects.
    MessageDelta(String),
    /// A piece of the agent's reasoning, as it reasons.
    ///
    /// Liveness, and more emphatically so than a message delta: a message is
    /// eventually committed and a thought never is. Nothing here is recorded,
    /// nothing is numbered, and a client that reloads sees none of it.
    ///
    /// That is not a limitation to be worked around. `Web_Style.md` §18.1
    /// requires a turn drawn from history and a turn watched arriving to
    /// produce the same result, so a durable thought is a thought that gets
    /// replayed — somebody reloading to check what the agent decided would
    /// watch it think a second time. The turn's own boundaries already say
    /// when the thinking happened.
    ThoughtDelta(String),
    /// The runtime process ended.
    Exited {
        /// Process exit code, when one was reported.
        code: Option<i32>,
        /// Why the session stopped.
        reason: StopReason,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_change_serializes_with_a_type_tag() {
        let e = EventKind::StatusChanged {
            status: SessionStatus::Ready,
            stop_reason: None,
            detail: None,
            capability: None,
        };
        let json = serde_json::to_value(&e).expect("serializes");
        assert_eq!(json["type"], "status_changed");
        assert_eq!(json["status"], "ready");
        assert!(
            json.get("stop_reason").is_none(),
            "absent reason must be omitted, not null"
        );
    }

    #[test]
    fn stop_reason_is_carried_when_present() {
        let e = EventKind::StatusChanged {
            status: SessionStatus::Idle,
            stop_reason: Some(StopReason::RequiresAction),
            detail: None,
            capability: None,
        };
        let json = serde_json::to_value(&e).expect("serializes");
        assert_eq!(json["stop_reason"], "requires_action");
    }

    /// One sample of every variant.
    ///
    /// The match is exhaustive on purpose. A variant added without being taught
    /// to [`KNOWN_TAGS`] stops compiling here, rather than shipping as an event
    /// this build writes and then refuses to read back.
    fn one_of_every_kind() -> Vec<EventKind> {
        let all = vec![
            EventKind::SessionCreated,
            EventKind::Progress {
                stage: crate::session::ProvisioningStage::Ready,
            },
            EventKind::StatusChanged {
                status: SessionStatus::Ready,
                stop_reason: None,
                detail: None,
                capability: None,
            },
            EventKind::TurnStarted,
            EventKind::TurnCompleted,
            EventKind::MessageCommitted {
                content: "hello".into(),
            },
            EventKind::OwnerMessage {
                content: "hello back".into(),
            },
            EventKind::ToolCallStarted {
                call_id: "call-1".into(),
                title: "read a file".into(),
                kind: "read".into(),
            },
            EventKind::ToolCallUpdated {
                call_id: "call-1".into(),
                status: "completed".into(),
            },
            EventKind::UsageReported {
                input_tokens: 1234,
                output_tokens: 56,
                context_used: 1290,
            },
            EventKind::ConversationGap { frames: 7 },
            EventKind::TerminalAttached,
            EventKind::TerminalDetached,
            EventKind::TerminalResized {
                columns: 80,
                rows: 24,
            },
            EventKind::ApprovalRequested {
                request_id: "r-1".into(),
                action: Some("run a command".into()),
                detail: None,
            },
            EventKind::ApprovalResolved {
                request_id: "r-1".into(),
                allowed: true,
            },
            EventKind::ConversationOpened {
                conversation_id: "conv-1".into(),
            },
            EventKind::PlaneChanged {
                from: crate::options::SessionMode::Terminal,
                to: crate::options::SessionMode::Conversation,
                snapshot_at: "2026-08-18T09:00:00Z".into(),
            },
        ];
        for kind in &all {
            match kind {
                EventKind::SessionCreated
                | EventKind::Progress { .. }
                | EventKind::StatusChanged { .. }
                | EventKind::TurnStarted
                | EventKind::TurnCompleted
                | EventKind::ConversationOpened { .. }
                | EventKind::MessageCommitted { .. }
                | EventKind::OwnerMessage { .. }
                | EventKind::ToolCallStarted { .. }
                | EventKind::ToolCallUpdated { .. }
                | EventKind::UsageReported { .. }
                | EventKind::ConversationGap { .. }
                | EventKind::TerminalAttached
                | EventKind::TerminalDetached
                | EventKind::TerminalResized { .. }
                | EventKind::ApprovalRequested { .. }
                | EventKind::ApprovalResolved { .. }
                | EventKind::PlaneChanged { .. } => {}
            }
        }
        all
    }

    #[test]
    fn every_variant_is_recognized_by_the_tag_it_serializes_to() {
        for kind in one_of_every_kind() {
            let json = serde_json::to_value(&kind).expect("serializes");
            let tag = json["type"].as_str().expect("carries a type tag");
            assert!(
                EventKind::recognizes_tag(tag),
                "{tag} is written by this build but would be read back as a stranger's"
            );
        }
    }

    #[test]
    fn every_variant_round_trips_through_its_serialized_form() {
        for kind in one_of_every_kind() {
            let json = serde_json::to_value(&kind).expect("serializes");
            let back: EventKind = serde_json::from_value(json.clone()).expect("deserializes");
            assert_eq!(
                serde_json::to_value(&back).expect("re-serializes"),
                json,
                "{kind:?} did not survive a round trip"
            );
        }
    }

    #[test]
    fn a_tag_this_build_does_not_own_is_not_recognized() {
        assert!(
            !EventKind::recognizes_tag("message_committed_by_a_newer_control_plane"),
            "an unknown discriminator must not be validated as one of ours"
        );
    }

    #[test]
    fn live_frames_have_no_sequence_field() {
        // Compile-time guarantee expressed as a behavioural test: the only way
        // to build a live frame carries no seq, so replay gaps are impossible.
        let f = LiveFrame::TerminalOutput(b"hello".to_vec());
        match f {
            LiveFrame::TerminalOutput(bytes) => assert_eq!(bytes, b"hello"),
            _ => panic!("unexpected frame"),
        }
    }
}
