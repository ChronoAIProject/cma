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
        /// Remaining fields in their extended-JSON representation.
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
    /// The runtime is blocked awaiting a decision from the client.
    ApprovalRequested {
        /// Correlates with the resolving decision.
        request_id: String,
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
    "terminal_attached",
    "terminal_detached",
    "terminal_resized",
    "approval_requested",
    "approval_resolved",
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
            },
            EventKind::TurnStarted,
            EventKind::TurnCompleted,
            EventKind::TerminalAttached,
            EventKind::TerminalDetached,
            EventKind::TerminalResized {
                columns: 80,
                rows: 24,
            },
            EventKind::ApprovalRequested {
                request_id: "r-1".into(),
            },
            EventKind::ApprovalResolved {
                request_id: "r-1".into(),
                allowed: true,
            },
        ];
        for kind in &all {
            match kind {
                EventKind::SessionCreated
                | EventKind::Progress { .. }
                | EventKind::StatusChanged { .. }
                | EventKind::TurnStarted
                | EventKind::TurnCompleted
                | EventKind::TerminalAttached
                | EventKind::TerminalDetached
                | EventKind::TerminalResized { .. }
                | EventKind::ApprovalRequested { .. }
                | EventKind::ApprovalResolved { .. } => {}
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
