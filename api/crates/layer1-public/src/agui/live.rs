//! Turning what is happening on a session into AG-UI events.
//!
//! The counterpart to `project`, and the two are deliberately not the same
//! thing. That one reads the record; this one watches the moment. Between a
//! turn starting and its message being committed the record says nothing at
//! all, so a client with only the log draws a working agent and a dead one
//! identically — which is the whole of what this exists to fix.
//!
//! **Nothing here carries a cursor**, and `AguiEvent::seq` being an `Option`
//! is what makes that structural rather than a rule somebody has to remember.
//! A liveness frame is what is happening rather than what happened: it is not
//! in the durable log, so a client cannot resume from it, and a frame given an
//! id would send a reconnecting client to a position the log has never held.
//!
//! **A client that missed these has missed nothing it needs.** The finished
//! message is committed durably and is waiting in the log; §7 of
//! `docs/API_AGUI.md` is what a client does about that, and it is unchanged.

use layer2_core::LiveFrame;

use super::project::AguiEvent;

/// The identifier the in-flight message travels under.
///
/// Deliberately not derived from a sequence, because there is not one yet: the
/// message has not been committed and has no place in the log. It is a fixed
/// name so successive pieces of one message land on one entry, and so the
/// committed message — which arrives under `msg_<seq>` — is a different
/// identity that a client replaces this with rather than appending to.
///
/// One in flight at a time, which is what a turn is.
const IN_FLIGHT: &str = "msg_live";

/// Project a liveness frame, where this protocol has a shape for one.
///
/// `None` for everything else, and silently: a terminal's bytes are a terminal
/// fact with no honest shape here, and a runtime exiting is already recorded
/// durably as a status change that the run stream carries in its own right.
/// Emitting either would put frames in a client's stream that mean nothing to
/// it, or say the same thing twice.
pub(super) fn project_live(frame: &LiveFrame, run_id: &str, thread_id: &str) -> Option<AguiEvent> {
    match frame {
        LiveFrame::MessageDelta(text) => Some(AguiEvent {
            kind: "TEXT_MESSAGE_CONTENT",
            run_id: run_id.to_string(),
            thread_id: thread_id.to_string(),
            body: serde_json::json!({ "messageId": IN_FLIGHT, "delta": text }),
            seq: None,
        }),
        LiveFrame::TerminalOutput(_) | LiveFrame::Exited { .. } => None,
    }
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod live_tests;
