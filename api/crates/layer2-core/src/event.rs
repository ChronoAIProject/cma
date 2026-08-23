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
use crate::session::StopReason;

// Re-exported so the split is a move rather than a rename: everything that
// named these through this module still does, and a reader looking for "what a
// durable event can say" is pointed at the file that holds it.
pub use crate::event_kinds::{AvailableCommand, ConfigOption, EventKind, PlanStep, ToolBody};

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
    /// The actions the owner may invoke right now.
    ///
    /// Liveness, not history: this describes the session as it is at this
    /// moment, and a list replayed from the log would offer actions that no
    /// longer exist. A client that missed it sees none rather than a stale one.
    AvailableCommands(Vec<AvailableCommand>),
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
