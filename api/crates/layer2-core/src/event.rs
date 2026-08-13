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

/// The semantic event vocabulary. Superset of what any single interface projects.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum EventKind {
    /// The session was created.
    SessionCreated,
    /// Session status changed.
    StatusChanged {
        /// The new status.
        status: SessionStatus,
        /// Why, when the change was a stop or a pause.
        #[serde(skip_serializing_if = "Option::is_none")]
        stop_reason: Option<StopReason>,
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
        };
        let json = serde_json::to_value(&e).expect("serializes");
        assert_eq!(json["stop_reason"], "requires_action");
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
