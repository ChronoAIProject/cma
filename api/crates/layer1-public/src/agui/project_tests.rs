//! What the durable log becomes on this interface.
//!
//! One projection per event kind, and one for every kind that must become
//! nothing. The second group is as load-bearing as the first: an event with no
//! honest shape here must be skipped rather than approximated, or a client's
//! stream fills with things that mean nothing to it.

use super::*;

/// A durable event as the log holds it.
fn logged(seq: u64, kind: EventKind) -> Event {
    Event {
        session_id: layer2_core::SessionId("sesn_1".into()),
        seq,
        kind,
    }
}

/// What one event projects to, for a run and a thread.
fn projected(kind: EventKind) -> Projected {
    project(&logged(7, kind), "run_1", "sesn_1")
}

/// The event types produced, in order.
fn kinds(p: &Projected) -> Vec<&'static str> {
    p.events.iter().map(|e| e.kind).collect()
}

#[test]
fn a_turn_beginning_starts_the_run() {
    let p = projected(EventKind::TurnStarted);
    assert_eq!(kinds(&p), vec!["RUN_STARTED"]);
    assert!(p.ending.is_none());
    assert_eq!(p.events[0].run_id, "run_1");
    assert_eq!(p.events[0].thread_id, "sesn_1");
}

#[test]
fn a_committed_message_arrives_whole_rather_than_as_pieces() {
    // The token-level pieces travelled as liveness and were never numbered, so
    // a client that missed them gets the message here. Sending only a content
    // event would leave a client with no boundary to close.
    let p = projected(EventKind::MessageCommitted {
        content: "carried across the pod boundary".into(),
    });
    assert_eq!(
        kinds(&p),
        vec![
            "TEXT_MESSAGE_START",
            "TEXT_MESSAGE_CONTENT",
            "TEXT_MESSAGE_END"
        ]
    );
    assert_eq!(p.events[1].body["delta"], "carried across the pod boundary");
    let id = p.events[0].body["messageId"].clone();
    assert!(
        p.events.iter().all(|e| e.body["messageId"] == id),
        "one message is one identifier"
    );
}

#[test]
fn a_message_identifier_is_stable_across_a_replay() {
    // A client deduplicating on message id must not see two messages because
    // it read the log twice. Deriving the id from the sequence is what makes
    // reading it again idempotent.
    let first = project(
        &logged(
            7,
            EventKind::MessageCommitted {
                content: "hi".into(),
            },
        ),
        "run_1",
        "sesn_1",
    );
    let again = project(
        &logged(
            7,
            EventKind::MessageCommitted {
                content: "hi".into(),
            },
        ),
        "run_2",
        "sesn_1",
    );
    assert_eq!(
        first.events[0].body["messageId"],
        again.events[0].body["messageId"]
    );
}

#[test]
fn a_tool_call_carries_the_runtimes_own_word_for_what_it_is() {
    // Namespaced rather than mapped onto the protocol's vocabulary: the sets
    // differ per kernel and grow, and a session must not fail because a kernel
    // invented a category.
    let p = projected(EventKind::ToolCallStarted {
        call_id: "call-1".into(),
        title: "read notes".into(),
        kind: "a-kind-this-build-has-never-seen".into(),
    });
    assert_eq!(kinds(&p), vec!["TOOL_CALL_START"]);
    assert_eq!(p.events[0].body["toolCallId"], "call-1");
    assert_eq!(p.events[0].body["toolCallName"], "read notes");
    assert_eq!(
        p.events[0].body["chrono:toolKind"],
        "a-kind-this-build-has-never-seen"
    );
}

#[test]
fn usage_arrives_as_a_state_patch_rather_than_a_call() {
    // So a chat surface can show spend without polling. The document's §3.
    let p = projected(EventKind::UsageReported {
        input_tokens: 412,
        output_tokens: 9,
        context_used: 1024,
    });
    assert_eq!(kinds(&p), vec!["STATE_DELTA"]);
    let delta = p.events[0].body["delta"].as_array().expect("a patch list");
    assert_eq!(delta.len(), 3);
    assert_eq!(delta[0]["path"], "/usage/inputTokens");
    assert_eq!(delta[0]["value"], 412);
}

#[test]
fn an_approval_ends_the_run_to_ask() {
    // The shape that surprises people, and the reason it is that shape: the
    // asking is durable, so a client that closed the tab finds it waiting
    // rather than having needed to hold a stream open.
    let p = projected(EventKind::ApprovalRequested {
        request_id: "apr_7Q2".into(),
        action: Some("command_execution".into()),
        detail: Some("Run `rm -rf build/`?".into()),
    });
    assert_eq!(kinds(&p), vec!["RUN_FINISHED"]);
    assert_eq!(p.ending, Some(Ending::Interrupted));
    let outcome = &p.events[0].body["outcome"];
    assert_eq!(outcome["type"], "interrupt");
    assert_eq!(outcome["interrupts"][0]["id"], "apr_7Q2");
    assert_eq!(
        outcome["interrupts"][0]["reason"],
        "chrono:command_execution"
    );
    assert_eq!(outcome["interrupts"][0]["message"], "Run `rm -rf build/`?");
}

#[test]
fn an_approval_with_nothing_to_say_still_asks() {
    // A runtime that offers no description of what it wants is still blocked
    // on somebody, and a client that got no interrupt would wait forever.
    let p = projected(EventKind::ApprovalRequested {
        request_id: "apr_1".into(),
        action: None,
        detail: None,
    });
    assert_eq!(p.ending, Some(Ending::Interrupted));
    assert_eq!(
        p.events[0].body["outcome"]["interrupts"][0]["reason"],
        "confirmation"
    );
}

#[test]
fn a_completed_turn_ends_the_run() {
    let p = projected(EventKind::TurnCompleted);
    assert_eq!(kinds(&p), vec!["RUN_FINISHED"]);
    assert_eq!(p.ending, Some(Ending::Finished));
    assert_eq!(p.events[0].body["outcome"]["type"], "complete");
}

#[test]
fn a_session_that_stops_working_says_so_rather_than_going_quiet() {
    // A client left on a stream that simply stops cannot tell that from an
    // agent taking a long time.
    for reason in [
        StopReason::Error,
        StopReason::Revoked,
        StopReason::BudgetReached,
    ] {
        let p = projected(EventKind::StatusChanged {
            status: SessionStatus::Idle,
            stop_reason: Some(reason),
            detail: Some("the model endpoint refused".into()),
            capability: None,
        });
        assert_eq!(kinds(&p), vec!["RUN_ERROR"], "for {reason:?}");
        assert_eq!(p.ending, Some(Ending::Failed), "for {reason:?}");
        assert_eq!(p.events[0].body["message"], "the model endpoint refused");
    }
}

#[test]
fn a_refusal_names_the_ability_the_session_lacked() {
    // `API_Overview.md` rule 4: errors name what is missing. The core arrives
    // at a refusal typed and knows which ability it was; a client handed only
    // "something went wrong" has been given the one part it cannot act on.
    let p = projected(EventKind::StatusChanged {
        status: SessionStatus::Terminated,
        stop_reason: Some(StopReason::Error),
        detail: Some("this session's runtime can no longer be what it is".into()),
        capability: Some("conversation".into()),
    });

    assert_eq!(kinds(&p), vec!["RUN_ERROR"]);
    assert_eq!(p.events[0].body["capability"], "conversation");
}

#[test]
fn a_failure_that_was_not_a_refusal_names_nothing() {
    // Most failures lack nothing — a provider that could not be reached is
    // not a session missing an ability. A name invented for those would be
    // read as one, and acted on.
    let p = projected(EventKind::StatusChanged {
        status: SessionStatus::Terminated,
        stop_reason: Some(StopReason::Error),
        detail: Some("could not provision a runtime".into()),
        capability: None,
    });

    assert!(
        p.events[0].body["capability"].is_null(),
        "got {}",
        p.events[0].body
    );
}

#[test]
fn a_session_being_moved_says_it_is_coming_back_and_a_failure_does_not() {
    // Both end the run, and the protocol has one event for that. `Web_Style`
    // §11 forbids a rotating session taking the failure ink, so a client has
    // to be able to tell them apart — and until this it could not.
    let moving = projected(EventKind::StatusChanged {
        status: SessionStatus::Rotating,
        stop_reason: None,
        detail: Some("moving this session to the other plane".into()),
        capability: None,
    });
    assert_eq!(
        moving.events[0].body["chrono:returns"], true,
        "a handover is not a failure: {}",
        moving.events[0].body
    );

    let failed = projected(EventKind::StatusChanged {
        status: SessionStatus::Terminated,
        stop_reason: Some(StopReason::Error),
        detail: Some("the runtime went away".into()),
        capability: None,
    });
    assert_eq!(
        failed.events[0].body["chrono:returns"], false,
        "and a failure is not a handover: {}",
        failed.events[0].body
    );
}

#[test]
fn going_idle_to_wait_for_a_person_does_not_end_the_run_twice() {
    // The approval already ended it with an interrupt outcome. A second
    // ending, as an error, would tell a client the session had failed when it
    // is waiting for them.
    let p = projected(EventKind::StatusChanged {
        status: SessionStatus::Idle,
        stop_reason: Some(StopReason::RequiresAction),
        detail: None,
        capability: None,
    });
    assert!(p.events.is_empty(), "got {:?}", kinds(&p));
    assert!(p.ending.is_none());
}

#[test]
fn a_lost_stretch_of_conversation_is_shown_as_a_boundary() {
    // Rather than a stream that silently skips, leaving its reader to notice.
    let p = projected(EventKind::ConversationGap { frames: 17 });
    assert_eq!(kinds(&p), vec!["STATE_DELTA"]);
    assert_eq!(p.events[0].body["delta"][0]["value"]["frames"], 17);
}

#[test]
fn what_this_interface_cannot_carry_becomes_nothing() {
    // The core's log is the union of what every interface needs. A resize, an
    // attachment, a screen — none has an honest shape here, and inventing one
    // would put events in a client's stream that mean nothing to it.
    for kind in [
        EventKind::TerminalAttached,
        EventKind::TerminalDetached,
        EventKind::TerminalResized {
            columns: 120,
            rows: 40,
        },
        EventKind::ApprovalResolved {
            request_id: "apr_1".into(),
            allowed: true,
        },
    ] {
        let p = project(&logged(7, kind.clone()), "run_1", "sesn_1");
        assert!(p.events.is_empty(), "{kind:?} projected {:?}", kinds(&p));
        assert!(p.ending.is_none(), "{kind:?} must not end the run");
    }
}

#[test]
fn the_sequence_rides_outside_the_payload() {
    // It is the SSE id, which is what a reconnecting client hands back. In the
    // body a client would be tempted to read it as protocol.
    let p = projected(EventKind::TurnStarted);
    assert_eq!(p.events[0].seq, 7);
    let rendered = serde_json::to_value(&p.events[0]).expect("serializes");
    assert!(rendered.get("seq").is_none(), "got {rendered}");
    assert_eq!(rendered["type"], "RUN_STARTED");
}

#[test]
fn a_plane_change_is_drawn_as_a_boundary() {
    // The surface changed under whoever is watching, and the snapshot moment
    // is where their history stops being continuous. A client that skips it
    // silently leaves its reader to notice the agent seems to have forgotten
    // something.
    let p = projected(EventKind::PlaneChanged {
        from: layer2_core::SessionMode::Terminal,
        to: layer2_core::SessionMode::Conversation,
        snapshot_at: "2026-08-18T09:00:00Z".into(),
    });

    assert_eq!(kinds(&p), vec!["STATE_DELTA"]);
    let value = &p.events[0].body["delta"][0]["value"];
    assert_eq!(value["from"], "terminal");
    assert_eq!(value["to"], "conversation");
    assert_eq!(value["snapshotAt"], "2026-08-18T09:00:00Z");
    assert!(
        p.ending.is_none(),
        "the change itself is history; what ends the run is the session going non-interactive"
    );
}

#[test]
fn a_session_being_moved_ends_the_run_rather_than_going_silent() {
    // The non-interactive window. A client left holding a stream that simply
    // stops cannot tell a handover from an agent taking its time, and the one
    // thing it must not do is keep sending turns at a runtime being torn down.
    let p = projected(EventKind::StatusChanged {
        status: SessionStatus::Rotating,
        stop_reason: None,
        detail: Some(
            "moving this session to the other plane; it will come back where it left off".into(),
        ),
        capability: None,
    });

    assert_eq!(kinds(&p), vec!["RUN_ERROR"]);
    assert!(matches!(p.ending, Some(Ending::Failed)));
    assert!(
        p.events[0].body["message"]
            .as_str()
            .expect("carries a message")
            .contains("come back"),
        "the message must say the session returns"
    );
}
