//! Turning the core's durable log into AG-UI events.
//!
//! A lossless projection in one direction: the log is the source, and nothing
//! here holds state a reconnecting client could not rebuild from it. That is
//! what makes a dropped stream recoverable — the events carry the same
//! sequence every other interface cursors on, so resuming is reading the log
//! again from where the last one stopped.
//!
//! ## Why some events become nothing
//!
//! The core's log is the union of what every interface needs, and this one
//! carries semantic events for a chat surface. A terminal resize, an
//! attachment taken, a screen captured — none has an honest AG-UI shape, and
//! inventing one would put events in a client's stream that mean nothing to
//! it. They are skipped, and skipped silently: a newer core recording
//! something new must not break a client that was written before it.
//!
//! ## Why a turn ending is not always the run ending
//!
//! This protocol ends a run to ask for permission and starts a new one to
//! resume, so an approval that arrives mid-turn closes the run with an
//! interrupt outcome rather than an ending. §4 of `docs/API_AGUI.md` is the
//! contract; this file is its implementation.

use layer2_core::{Event, EventKind, SessionStatus, StopReason};
use serde::Serialize;

/// One AG-UI event, ready to travel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct AguiEvent {
    /// The event type, in the protocol's own spelling.
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// The run this belongs to, echoed on every event.
    #[serde(rename = "runId")]
    pub run_id: String,
    /// The session, which this protocol calls a thread.
    #[serde(rename = "threadId")]
    pub thread_id: String,
    /// Everything else the event carries.
    #[serde(flatten)]
    pub body: serde_json::Value,
    /// The durable sequence this was projected from.
    ///
    /// Not part of the protocol's own payload: it rides as the SSE id, which
    /// is what a reconnecting client hands back. Skipped in the body so a
    /// client is never tempted to read it as protocol.
    #[serde(skip)]
    pub seq: u64,
}

/// Whether a run ends here, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Ending {
    /// The turn finished. The run ends with it.
    Finished,
    /// The runtime is blocked on a decision. The run ends to ask.
    Interrupted,
    /// The session failed. The run ends saying so.
    Failed,
}

/// What one durable event becomes, and whether the run continues past it.
pub(super) struct Projected {
    /// The events to send, in order. Empty where the log records something
    /// this interface has no honest shape for.
    pub events: Vec<AguiEvent>,
    /// Set where this event ends the run.
    pub ending: Option<Ending>,
}

impl Projected {
    fn nothing() -> Self {
        Self {
            events: Vec::new(),
            ending: None,
        }
    }
}

/// Project one durable event.
pub(super) fn project(event: &Event, run_id: &str, thread_id: &str) -> Projected {
    let make = |kind: &'static str, body: serde_json::Value| AguiEvent {
        kind,
        run_id: run_id.to_string(),
        thread_id: thread_id.to_string(),
        body,
        seq: event.seq,
    };

    match &event.kind {
        EventKind::TurnStarted => Projected {
            events: vec![make("RUN_STARTED", serde_json::json!({}))],
            ending: None,
        },
        // A committed message is the whole thing the agent said. The pieces it
        // was assembled from travelled as liveness and were never numbered, so
        // a client that missed them gets the message here rather than a gap.
        EventKind::MessageCommitted { content } => Projected {
            events: vec![
                make(
                    "TEXT_MESSAGE_START",
                    serde_json::json!({ "messageId": message_id(event.seq), "role": "assistant" }),
                ),
                make(
                    "TEXT_MESSAGE_CONTENT",
                    serde_json::json!({ "messageId": message_id(event.seq), "delta": content }),
                ),
                make(
                    "TEXT_MESSAGE_END",
                    serde_json::json!({ "messageId": message_id(event.seq) }),
                ),
            ],
            ending: None,
        },
        EventKind::ToolCallStarted {
            call_id,
            title,
            kind,
        } => Projected {
            events: vec![make(
                "TOOL_CALL_START",
                // `kind` is the runtime's own word for what sort of tool this
                // is. Carried through under a namespaced key rather than
                // mapped onto the protocol's vocabulary: the sets differ per
                // kernel and grow, and a client that recognises one renders it.
                serde_json::json!({
                    "toolCallId": call_id,
                    "toolCallName": title,
                    "chrono:toolKind": kind,
                }),
            )],
            ending: None,
        },
        EventKind::ToolCallUpdated { call_id, status } => Projected {
            events: vec![make(
                "TOOL_CALL_END",
                serde_json::json!({ "toolCallId": call_id, "chrono:status": status }),
            )],
            ending: None,
        },
        // Usage arrives as a state patch rather than a separate call, so a
        // chat surface can show spend without polling. §3 of the document.
        EventKind::UsageReported {
            input_tokens,
            output_tokens,
            context_used,
        } => Projected {
            events: vec![make(
                "STATE_DELTA",
                serde_json::json!({ "delta": [
                    { "op": "replace", "path": "/usage/inputTokens", "value": input_tokens },
                    { "op": "replace", "path": "/usage/outputTokens", "value": output_tokens },
                    { "op": "replace", "path": "/usage/contextUsed", "value": context_used },
                ]}),
            )],
            ending: None,
        },
        // The run ends to ask. Not a mid-stream prompt: a client that closed
        // the tab finds this still pending and answerable, which only works
        // because the asking is durable rather than held on a connection.
        EventKind::ApprovalRequested {
            request_id,
            action,
            detail,
        } => Projected {
            events: vec![make(
                "RUN_FINISHED",
                serde_json::json!({ "outcome": {
                    "type": "interrupt",
                    "interrupts": [{
                        "id": request_id,
                        "reason": reason_for(action.as_deref()),
                        "message": detail.clone().or_else(|| action.clone()),
                    }],
                }}),
            )],
            ending: Some(Ending::Interrupted),
        },
        EventKind::TurnCompleted => Projected {
            events: vec![make(
                "RUN_FINISHED",
                serde_json::json!({ "outcome": { "type": "complete" } }),
            )],
            ending: Some(Ending::Finished),
        },
        // A session that failed or was stopped ends the run saying so. A
        // client left waiting on a stream that simply stops cannot tell that
        // from a slow agent.
        EventKind::StatusChanged {
            status,
            stop_reason,
            detail,
            capability,
        } => match ends_the_run(*status, *stop_reason) {
            true => Projected {
                events: vec![make(
                    "RUN_ERROR",
                    serde_json::json!({
                        "message": detail.clone().unwrap_or_else(|| "the session stopped".into()),
                        "code": stop_reason.map(wire_stop_reason),
                        // The ability the session lacked, when that is what
                        // stopped it. The core arrives at a refusal typed and
                        // knows the name; a client shown "something went
                        // wrong" has been handed the one part it could not
                        // act on. Absent for every failure that was not a
                        // refusal, which is most of them.
                        "capability": capability,
                        // Whether the session is coming back. A rotation ends
                        // the run — its runtime is being torn down — but
                        // nothing has gone wrong, and a client that cannot
                        // tell the two apart draws a handover as a failure.
                        // The protocol has no third lifecycle event, so this
                        // rides as an extension rather than as an invented
                        // one; `Web_Style.md` §11 forbids a rotating session
                        // taking the failure ink, and this is what a client
                        // distinguishes on.
                        "chrono:returns": *status == SessionStatus::Rotating,
                    }),
                )],
                ending: Some(Ending::Failed),
            },
            false => Projected::nothing(),
        },
        // Part of the conversation was dropped rather than recorded. Shown as
        // a boundary rather than hidden, in the same spirit as the log itself:
        // a client that skips silently leaves its reader to notice.
        EventKind::ConversationGap { frames } => Projected {
            events: vec![make(
                "STATE_DELTA",
                serde_json::json!({ "delta": [
                    { "op": "add", "path": "/gaps/-", "value": { "frames": frames } },
                ]}),
            )],
            ending: None,
        },
        /*
         * The surface changed under whoever is watching. Drawn as a boundary
         * for the same reason a dropped stretch of conversation is: the
         * snapshot moment is where history stops being continuous, and a
         * client that skips it silently leaves its reader to notice that the
         * agent seems to have forgotten something.
         *
         * A state delta rather than an invented event kind, matching how a gap
         * is already carried, so a client needs no new vocabulary to read it.
         */
        EventKind::PlaneChanged {
            from,
            to,
            snapshot_at,
        } => Projected {
            events: vec![make(
                "STATE_DELTA",
                serde_json::json!({ "delta": [
                    { "op": "add", "path": "/planeChanges/-", "value": {
                        "from": from,
                        "to": to,
                        "snapshotAt": snapshot_at,
                    } },
                ]}),
            )],
            ending: None,
        },
        // Recorded for the log's own sake and carried by other interfaces. No
        // honest AG-UI shape exists for a resize or an attachment, and an
        // approval's resolution is already known to whoever sent it.
        _ => Projected::nothing(),
    }
}

/// A message identifier derived from the sequence that carried it.
///
/// Stable across a replay, which is what a client deduplicating on message id
/// depends on: reading the log twice must not produce two messages.
fn message_id(seq: u64) -> String {
    format!("msg_{seq}")
}

/// The protocol's word for why a run was interrupted.
///
/// The standard set does not cover what a coding agent asks about, so the
/// document reserves a `chrono:` prefix for the rest. The runtime's own word
/// is carried rather than mapped, because the sets differ per kernel.
fn reason_for(action: Option<&str>) -> String {
    match action {
        Some(action) => format!("chrono:{action}"),
        None => "confirmation".to_string(),
    }
}

/// Whether a status change is the end of the run.
///
/// Going idle is not: a turn ending already says so, and an approval parks a
/// session idle while its run is being asked about. What ends a run is the
/// session ceasing to be able to answer at all.
fn ends_the_run(status: SessionStatus, stop_reason: Option<StopReason>) -> bool {
    match status {
        SessionStatus::Terminated => true,
        // A handover, either onto a newer runtime or onto the other plane. The
        // session is coming back, but this run is not: its runtime is being
        // torn down, and a client left holding a stream that simply stops
        // cannot tell that from an agent taking its time. The detail carried
        // with the status says which handover it is and that the session
        // returns, so ending the run here is a statement rather than a
        // failure dressed up as one.
        SessionStatus::Rotating => true,
        SessionStatus::Idle => matches!(
            stop_reason,
            Some(StopReason::Error) | Some(StopReason::Revoked) | Some(StopReason::BudgetReached)
        ),
        _ => false,
    }
}

/// A stop reason as the wire spells it.
fn wire_stop_reason(reason: StopReason) -> String {
    serde_json::to_value(reason)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
#[path = "project_tests.rs"]
mod project_tests;
