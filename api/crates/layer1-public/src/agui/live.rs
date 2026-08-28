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

/// The identifier the agent's reasoning travels under.
///
/// Distinct from the in-flight message for the same reason that one is
/// distinct from the committed message: they are different things and a client
/// upserting by identifier must not merge them. Reasoning is never committed
/// at all, so this identifier never becomes a durable one.
const REASONING: &str = "reasoning_live";

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
            timestamp: None,
            seq: None,
            live: true,
        }),
        /*
         * `REASONING_MESSAGE_CONTENT`, and only it.
         *
         * The protocol also has `REASONING_START` and `REASONING_END`, and
         * they are deliberately not emitted: this lane sees a stream of
         * thoughts and has no view of where a turn begins or ends, so a
         * bracket sent from here would be a guess. The run's own lifecycle
         * events already say when the thinking happened, and a client clears
         * reasoning when the run ends. `docs/API_AGUI.md` says so rather than
         * leaving a client to infer it from the absence.
         *
         * The `THINKING_*` family is the deprecated spelling of these. It is
         * declared in the capability document as false rather than omitted,
         * because a client written against the older names needs to be told
         * they are not coming rather than left to time out on them.
         */
        LiveFrame::ThoughtDelta(text) => Some(AguiEvent {
            kind: "REASONING_MESSAGE_CONTENT",
            run_id: run_id.to_string(),
            thread_id: thread_id.to_string(),
            body: serde_json::json!({ "messageId": REASONING, "delta": text }),
            timestamp: None,
            seq: None,
            live: true,
        }),
        // The actions the owner may invoke, as a state patch on the live lane.
        //
        // This once said the list never rides the log, and it no longer holds:
        // the same list is recorded durably and projected onto this very path
        // from the log. The reasoning was that a replayed list could offer
        // actions that no longer exist, and the outcome was that it reached
        // nobody at all — the kernel announces it as the conversation opens,
        // before any client has subscribed.
        //
        // Both lanes write the same path deliberately. A client that was
        // watching and one that reloaded must hold one list, not two.
        LiveFrame::AvailableCommands(commands) => Some(AguiEvent {
            kind: "STATE_DELTA",
            run_id: run_id.to_string(),
            thread_id: thread_id.to_string(),
            body: serde_json::json!({ "delta": [
                { "op": "replace", "path": "/availableCommands", "value": commands
                    .iter()
                    .map(|c| serde_json::json!({ "name": c.name, "description": c.description }))
                    .collect::<Vec<_>>()
                },
            ]}),
            timestamp: None,
            seq: None,
            live: true,
        }),
        LiveFrame::TerminalOutput(_) | LiveFrame::Exited { .. } => None,
    }
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod live_tests;
