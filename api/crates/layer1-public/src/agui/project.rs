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

use layer2_core::{Event, EventKind, SessionStatus};
// Re-exported so the split is a move rather than a rename. `live.rs`,
// `run.rs` and `replay.rs` all refer to `project::AguiEvent` and
// `project::sse_frame`, and this is what keeps them doing so.
pub(super) use super::frame::{sse_frame, AguiEvent};
use super::run_ending::{ends_the_run, wire_stop_reason, Ending};

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

    /// Make completion of this projection the durable resume point.
    ///
    /// Applied after the exhaustive event-kind match, so every current and
    /// future projection arm inherits the invariant without remembering to
    /// stamp its own frames. Clearing the leading frames here also protects the
    /// rule if an arm constructs an `AguiEvent` without using `make`.
    fn with_final_cursor(mut self, seq: u64) -> Self {
        let Some((last, leading)) = self.events.split_last_mut() else {
            return self;
        };
        for frame in leading {
            frame.seq = None;
        }
        last.seq = Some(seq);
        self
    }
}

/// Project one durable event.
pub(super) fn project(event: &Event, run_id: &str, thread_id: &str) -> Projected {
    // A durable event's time is the one the log stamped when it was appended;
    // empty means a control plane that predates the field, and that is carried
    // as absence rather than as a made-up moment.
    let timestamp = (!event.appended_at.is_empty()).then(|| event.appended_at.clone());
    let make = |kind: &'static str, body: serde_json::Value| AguiEvent {
        kind,
        run_id: run_id.to_string(),
        thread_id: thread_id.to_string(),
        body,
        timestamp: timestamp.clone(),
        seq: None,
        live: false,
    };

    let projected = match &event.kind {
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
        // The owner's side of the conversation, projected as a user turn. It is
        // durable for exactly this reason: a replayed conversation shows both
        // sides, and the owner's turn is the one §18.1 labels with its time.
        EventKind::OwnerMessage { content } => Projected {
            events: vec![
                make(
                    "TEXT_MESSAGE_START",
                    serde_json::json!({ "messageId": message_id(event.seq), "role": "user" }),
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
            arguments,
        } => {
            let mut events = vec![make(
                "TOOL_CALL_START",
                // `kind` is the runtime's own word for what sort of tool this
                // is. Carried through under a namespaced key rather than
                // mapped onto the protocol's vocabulary: the sets differ per
                // kernel and grow, and a client that recognises one renders it.
                serde_json::json!({
                    "toolCallId": call_id,
                    "toolCallName": title,
                    "cma:toolKind": kind,
                }),
            )];
            if let Some(arguments) = arguments {
                events.push(make(
                    "TOOL_CALL_ARGS",
                    serde_json::json!({
                        "toolCallId": call_id,
                        "delta": arguments.text,
                        "cma:truncated": arguments.truncated,
                    }),
                ));
            }
            Projected {
                events,
                ending: None,
            }
        }
        EventKind::ToolCallUpdated {
            call_id,
            status,
            result,
        } => {
            let mut events = vec![make(
                "TOOL_CALL_END",
                serde_json::json!({ "toolCallId": call_id, "cma:status": status }),
            )];
            if let Some(result) = result {
                events.push(make(
                    "TOOL_CALL_RESULT",
                    serde_json::json!({
                        "toolCallId": call_id,
                        "content": result.text,
                        "cma:truncated": result.truncated,
                    }),
                ));
            }
            Projected {
                events,
                ending: None,
            }
        }
        // Usage arrives as a state patch rather than a separate call, so a
        // chat surface can show spend without polling. §3 of the document.
        EventKind::UsageReported {
            input_tokens,
            output_tokens,
            context_used,
            context_window,
        } => {
            let mut delta = vec![
                serde_json::json!({ "op": "replace", "path": "/usage/inputTokens", "value": input_tokens }),
                serde_json::json!({ "op": "replace", "path": "/usage/outputTokens", "value": output_tokens }),
            ];
            // Turn accounting uses zero to mean that it carried no context
            // reading. Keep the initial or last real reading in that case.
            if *context_used > 0 {
                delta.push(serde_json::json!({
                    "op": "replace",
                    "path": "/usage/contextUsed",
                    "value": context_used,
                }));
            }
            if let Some(window) = context_window {
                delta.push(serde_json::json!({
                    "op": "replace",
                    "path": "/usage/contextWindow",
                    "value": window,
                }));
            }
            Projected {
                events: vec![make("STATE_DELTA", serde_json::json!({ "delta": delta }))],
                ending: None,
            }
        }
        EventKind::ContextCompacted { was, now, window } => {
            let value = match window {
                Some(window) => serde_json::json!({ "was": was, "now": now, "window": window }),
                None => serde_json::json!({ "was": was, "now": now }),
            };
            Projected {
                events: vec![make(
                    "STATE_DELTA",
                    serde_json::json!({ "delta": [
                        { "op": "add", "path": "/contextCompactions/-", "value": value },
                    ]}),
                )],
                ending: None,
            }
        }
        // A permission-mode change is a safety matter and is surfaced as state,
        // not as prose: the surface draws the new mode as a fact about the
        // session rather than as something the agent said.
        EventKind::ModeChanged { mode } => Projected {
            events: vec![make(
                "STATE_DELTA",
                serde_json::json!({ "delta": [
                    { "op": "replace", "path": "/mode", "value": mode },
                ]}),
            )],
            ending: None,
        },
        // The agent's plan, as a state patch rather than an invented event.
        // A revision replaces the whole plan, which is what a `replace` on the
        // list path says to a client.
        EventKind::PlanSet { entries } => Projected {
            events: vec![make(
                "STATE_DELTA",
                serde_json::json!({ "delta": [
                    { "op": "replace", "path": "/plan", "value": entries
                        .iter()
                        .map(|step| serde_json::json!({ "title": step.title, "status": step.status }))
                        .collect::<Vec<_>>()
                    },
                ]}),
            )],
            ending: None,
        },
        // The settings the kernel says are settable, as a state patch so a
        // client can render a selection rather than a free-text box.
        EventKind::ConfigOptionsSet { options } => Projected {
            events: vec![make(
                "STATE_DELTA",
                serde_json::json!({ "delta": [
                    { "op": "replace", "path": "/configOptions", "value": options
                        .iter()
                        .map(|o| serde_json::json!({ "id": o.id, "name": o.name, "kind": o.kind }))
                        .collect::<Vec<_>>()
                    },
                ]}),
            )],
            ending: None,
        },
        // The actions the owner may invoke, on the same state path the live
        // lane writes — so a client that replays the log and one that was
        // already watching arrive at the same list rather than two.
        EventKind::AvailableCommandsSet { commands } => Projected {
            events: vec![make(
                "STATE_DELTA",
                serde_json::json!({ "delta": [
                    { "op": "replace", "path": "/availableCommands", "value": commands
                        .iter()
                        .map(|c| serde_json::json!({ "name": c.name, "description": c.description }))
                        .collect::<Vec<_>>()
                    },
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
        } => {
            let reason = reason_for(action.as_deref());
            let message = detail.clone().or_else(|| action.clone());
            Projected {
                events: vec![
                    make(
                        "RUN_FINISHED",
                        serde_json::json!({ "outcome": {
                            "type": "interrupt",
                            "interrupts": [{
                                "id": request_id,
                                "reason": reason,
                                "message": message,
                            }],
                        }}),
                    ),
                    make(
                        "STATE_DELTA",
                        serde_json::json!({ "delta": [{
                            "op": "add",
                            "path": pending_approval_path(request_id),
                            "value": {
                                "id": request_id,
                                "reason": reason,
                                "message": message,
                            },
                        }] }),
                    ),
                ],
                ending: Some(Ending::Interrupted),
            }
        }
        EventKind::ApprovalResolved { request_id, .. } => Projected {
            events: vec![make(
                "STATE_DELTA",
                serde_json::json!({ "delta": [{
                    "op": "remove",
                    "path": pending_approval_path(request_id),
                }] }),
            )],
            ending: None,
        },
        EventKind::TurnCompleted => Projected {
            events: vec![make(
                "RUN_FINISHED",
                serde_json::json!({ "outcome": { "type": "complete" } }),
            )],
            ending: Some(Ending::Finished),
        },
        // Provisioning is durable, including while a rotation's replacement
        // runtime starts. Project the Layer 2 value itself so AG-UI and REST
        // cannot acquire two spellings for the same stage.
        EventKind::Progress { stage } => Projected {
            events: vec![make(
                "STATE_DELTA",
                serde_json::json!({ "delta": [{
                    "op": "replace",
                    "path": "/progress",
                    "value": stage,
                }] }),
            )],
            ending: None,
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
                        "cma:returns": *status == SessionStatus::Rotating,
                    }),
                )],
                ending: Some(Ending::Failed),
            },
            // The Layer 2 value is the REST value too. Serializing it here,
            // rather than mapping variants to local strings, keeps one status
            // vocabulary across both interfaces.
            false => Projected {
                events: vec![make(
                    "STATE_DELTA",
                    serde_json::json!({ "delta": [{
                        "op": "replace",
                        "path": "/status",
                        "value": status,
                    }] }),
                )],
                ending: None,
            },
        },
        /*
         * The agent is waiting for its owner to sign in to their own account.
         *
         * A state delta rather than a new event kind, matching how a gap and a
         * plane change already travel, so a client needs no new vocabulary to
         * read it. It is state rather than conversation: nobody said this, and
         * putting it in the message stream would mix a thing the reader must
         * act on into the record of what the agent produced.
         *
         * What travels is an address and a short-lived code. The credential
         * the sign-in produces is written by the agent inside its own sandbox
         * and never reaches this service, which is the whole basis on which an
         * own-account session is offered at all.
         */
        EventKind::SignInRequired { url, code, message } => Projected {
            events: vec![make(
                "STATE_DELTA",
                serde_json::json!({ "delta": [
                    { "op": "replace", "path": "/signIn", "value": {
                        "url": url,
                        "code": code,
                        "message": message,
                    }},
                ]}),
            )],
            ending: None,
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
        /*
         * The runtime under this session died and was replaced.
         *
         * A state delta rather than an invented event kind, matching how a
         * gap and a plane change already travel, so a client needs no new
         * vocabulary to read it. Appended rather than replaced: a session can
         * lose more than one runtime, and each loss is a separate thing that
         * happened.
         *
         * `noticedAt` and not `at`. It is the moment the service *established*
         * the loss, which is up to one reconciliation sweep after the runtime
         * actually stopped. Naming it as the death would be a claim nothing
         * here can support.
         */
        EventKind::RuntimeLost { runtime_id, cause } => Projected {
            events: vec![make(
                "STATE_DELTA",
                serde_json::json!({ "delta": [
                    { "op": "add", "path": "/runtimeLosses/-", "value": {
                        "runtime": runtime_id,
                        "cause": cause,
                        "noticedAt": timestamp.clone(),
                    } },
                ]}),
            )],
            ending: None,
        },
        /*
         * An episode is durable state, unlike the readings it was folded
         * from. A reader who reloads must still see an open squeeze, so this
         * belongs in `project` and carries the log's cursor and timestamp.
         *
         * It does not end the run: pressure is something to act on while the
         * agent remains working, not a failure in the run lifecycle.
         */
        EventKind::MemoryPressureOpened {
            used_bytes,
            limit_bytes,
            occupancy_permille,
        } => Projected {
            events: vec![make(
                "STATE_DELTA",
                serde_json::json!({ "delta": [{
                    "op": "replace",
                    "path": "/memoryPressure",
                    "value": {
                        "usedBytes": used_bytes,
                        "limitBytes": limit_bytes,
                        "occupancyPermille": occupancy_permille,
                        "since": timestamp.clone(),
                    },
                }] }),
            )],
            ending: None,
        },
        /*
         * Replace with `null`, never remove. RFC 6902 rejects removing a path
         * a mid-session reader never received, whereas null is a value every
         * client can apply idempotently.
         */
        EventKind::MemoryPressureClosed { .. } => Projected {
            events: vec![make(
                "STATE_DELTA",
                serde_json::json!({ "delta": [{
                    "op": "replace",
                    "path": "/memoryPressure",
                    "value": null,
                }] }),
            )],
            ending: None,
        },
        // Recorded for the log's own sake and carried by other interfaces. No
        // honest AG-UI shape exists for a resize or an attachment.
        _ => Projected::nothing(),
    };
    projected.with_final_cursor(event.seq)
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
/// document reserves a `cma:` prefix for the rest. The runtime's own word
/// is carried rather than mapped, because the sets differ per kernel.
fn reason_for(action: Option<&str>) -> String {
    match action {
        Some(action) => format!("cma:{action}"),
        None => "confirmation".to_string(),
    }
}

/// The dynamic state member owned by one approval request.
fn pending_approval_path(request_id: &str) -> String {
    format!(
        "/pendingApprovals/{}",
        escape_json_pointer_token(request_id)
    )
}

/// Encode one RFC 6901 reference token, in the order the RFC requires.
fn escape_json_pointer_token(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
#[path = "project_tests.rs"]
mod project_tests;
