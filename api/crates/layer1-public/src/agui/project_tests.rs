//! What the durable log becomes on this interface.
//!
//! One projection per event kind, and one for every kind that must become
//! nothing. The second group is as load-bearing as the first: an event with no
//! honest shape here must be skipped rather than approximated, or a client's
//! stream fills with things that mean nothing to it.

use super::*;
use layer2_core::{session::ProvisioningStage, StopReason};

/// A durable event as the log holds it.
fn logged(seq: u64, kind: EventKind) -> Event {
    Event {
        session_id: layer2_core::SessionId("sesn_1".into()),
        seq,
        kind,
        appended_at: String::new(),
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

/// Assert the resume point is projection completion, never a leading frame.
fn assert_only_final_carries_cursor(projected: &Projected) {
    let (last, leading) = projected
        .events
        .split_last()
        .expect("the tested event has an AG-UI projection");
    assert!(
        leading.iter().all(|frame| frame.seq.is_none()),
        "leading projection frames must not advance replay: {:?}",
        leading
            .iter()
            .map(|frame| (frame.kind, frame.seq))
            .collect::<Vec<_>>()
    );
    assert_eq!(last.seq, Some(7), "the final frame completes sequence 7");
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
    assert_only_final_carries_cursor(&p);
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
fn an_owner_message_arrives_as_a_user_turn() {
    let p = projected(EventKind::OwnerMessage {
        content: "open a pull request".into(),
    });
    assert_eq!(
        kinds(&p),
        vec![
            "TEXT_MESSAGE_START",
            "TEXT_MESSAGE_CONTENT",
            "TEXT_MESSAGE_END"
        ]
    );
    assert_eq!(p.events[0].body["role"], "user");
    assert_eq!(p.events[1].body["delta"], "open a pull request");
    let id = p.events[0].body["messageId"].clone();
    assert!(
        p.events.iter().all(|e| e.body["messageId"] == id),
        "one message is one identifier"
    );
    assert_only_final_carries_cursor(&p);
}

#[test]
fn a_durable_event_carries_its_append_time() {
    let mut event = logged(7, EventKind::TurnStarted);
    event.appended_at = "2026-08-19T04:00:00Z".into();
    let p = project(&event, "run_1", "sesn_1");
    assert_eq!(
        p.events[0].timestamp.as_deref(),
        Some("2026-08-19T04:00:00Z")
    );
}

#[test]
fn an_event_without_an_append_time_carries_no_timestamp() {
    // A control plane that predates the field wrote no time; that is absence,
    // never an invented moment.
    let p = projected(EventKind::TurnStarted);
    assert!(p.events[0].timestamp.is_none());
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
        arguments: None,
    });
    assert_eq!(kinds(&p), vec!["TOOL_CALL_START"]);
    assert_eq!(p.events[0].seq, Some(7));
    assert_eq!(p.events[0].body["toolCallId"], "call-1");
    // `toolCallName` is the protocol's own field, and this assertion was
    // always right. What nothing checked was whether the *client* read it —
    // it read `target`, a name nothing sends, so every tool call reached the
    // surface without the one thing that identifies it. The seam is held from
    // the other side too, in `web/src/agui/wire.test.ts`.
    assert_eq!(p.events[0].body["toolCallName"], "read notes");
    assert_eq!(
        p.events[0].body["cma:toolKind"],
        "a-kind-this-build-has-never-seen"
    );
}

#[test]
fn a_tool_call_start_with_arguments_advances_only_after_the_arguments() {
    let p = projected(EventKind::ToolCallStarted {
        call_id: "call-1".into(),
        title: "read notes".into(),
        kind: "read".into(),
        arguments: Some(layer2_core::event::ToolBody {
            text: r#"{"path":"notes.md"}"#.into(),
            truncated: false,
        }),
    });

    assert_eq!(kinds(&p), vec!["TOOL_CALL_START", "TOOL_CALL_ARGS"]);
    assert_only_final_carries_cursor(&p);
}

#[test]
fn a_tool_call_update_with_a_result_advances_only_after_the_result() {
    let p = projected(EventKind::ToolCallUpdated {
        call_id: "call-1".into(),
        status: "completed".into(),
        result: Some(layer2_core::event::ToolBody {
            text: "the notes".into(),
            truncated: false,
        }),
    });

    assert_eq!(kinds(&p), vec!["TOOL_CALL_END", "TOOL_CALL_RESULT"]);
    assert_only_final_carries_cursor(&p);
}

#[test]
fn a_tool_call_update_without_a_result_advances_on_the_end() {
    let p = projected(EventKind::ToolCallUpdated {
        call_id: "call-1".into(),
        status: "completed".into(),
        result: None,
    });

    assert_eq!(kinds(&p), vec!["TOOL_CALL_END"]);
    assert_eq!(p.events[0].seq, Some(7));
}

#[test]
fn usage_arrives_as_a_state_patch_rather_than_a_call() {
    // So a chat surface can show spend without polling. The document's §3.
    let p = projected(EventKind::UsageReported {
        input_tokens: 412,
        output_tokens: 9,
        context_used: 1024,
        context_window: Some(200_000),
    });
    assert_eq!(kinds(&p), vec!["STATE_DELTA"]);
    let delta = p.events[0].body["delta"].as_array().expect("a patch list");
    assert_eq!(delta.len(), 4);
    assert_eq!(delta[0]["path"], "/usage/inputTokens");
    assert_eq!(delta[0]["value"], 412);
    assert_eq!(delta[3]["path"], "/usage/contextWindow");
    assert_eq!(delta[3]["value"], 200_000);
}

#[test]
fn a_usage_report_without_a_window_does_not_erase_one() {
    let p = projected(EventKind::UsageReported {
        input_tokens: 412,
        output_tokens: 9,
        context_used: 1024,
        context_window: None,
    });
    let delta = p.events[0].body["delta"].as_array().expect("a patch list");

    assert_eq!(delta.len(), 3);
    assert!(delta
        .iter()
        .all(|patch| patch["path"] != "/usage/contextWindow"));
}

#[test]
fn turn_accounting_without_a_context_reading_does_not_erase_one() {
    let p = projected(EventKind::UsageReported {
        input_tokens: 0,
        output_tokens: 0,
        context_used: 0,
        context_window: None,
    });
    let delta = p.events[0].body["delta"].as_array().expect("a patch list");

    assert_eq!(delta.len(), 2);
    assert_eq!(delta[0]["path"], "/usage/inputTokens");
    assert_eq!(delta[0]["value"], 0);
    assert_eq!(delta[1]["path"], "/usage/outputTokens");
    assert_eq!(delta[1]["value"], 0);
    assert!(delta
        .iter()
        .all(|patch| patch["path"] != "/usage/contextUsed"));
}

#[test]
fn a_context_compaction_is_appended_to_state_without_an_invented_event_kind() {
    let p = projected(EventKind::ContextCompacted {
        was: 198_000,
        now: 42_100,
        window: None,
    });

    assert_eq!(kinds(&p), vec!["STATE_DELTA"]);
    let patch = &p.events[0].body["delta"][0];
    assert_eq!(patch["op"], "add");
    assert_eq!(patch["path"], "/contextCompactions/-");
    assert_eq!(patch["value"]["was"], 198_000);
    assert_eq!(patch["value"]["now"], 42_100);
    assert!(patch["value"].get("window").is_none());
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
        locator: None,
    });
    assert_eq!(kinds(&p), vec!["RUN_FINISHED", "STATE_DELTA"]);
    assert_eq!(p.ending, Some(Ending::Interrupted));
    let outcome = &p.events[0].body["outcome"];
    assert_eq!(outcome["type"], "interrupt");
    assert_eq!(outcome["interrupts"][0]["id"], "apr_7Q2");
    assert_eq!(outcome["interrupts"][0]["reason"], "cma:command_execution");
    assert_eq!(outcome["interrupts"][0]["message"], "Run `rm -rf build/`?");
    let patch = &p.events[1].body["delta"][0];
    assert_eq!(patch["op"], "add");
    assert_eq!(patch["path"], "/pendingApprovals/apr_7Q2");
    assert_eq!(patch["value"]["id"], "apr_7Q2");
    assert_eq!(patch["value"]["reason"], "cma:command_execution");
    assert_eq!(patch["value"]["message"], "Run `rm -rf build/`?");
    assert_only_final_carries_cursor(&p);
}

#[test]
fn an_approval_with_nothing_to_say_still_asks() {
    // A runtime that offers no description of what it wants is still blocked
    // on somebody, and a client that got no interrupt would wait forever.
    let p = projected(EventKind::ApprovalRequested {
        request_id: "apr_1".into(),
        action: None,
        detail: None,
        locator: None,
    });
    assert_eq!(p.ending, Some(Ending::Interrupted));
    assert_eq!(
        p.events[0].body["outcome"]["interrupts"][0]["reason"],
        "confirmation"
    );
    assert!(p.events[1].body["delta"][0]["value"]["message"].is_null());
}

#[test]
fn an_approval_id_is_one_escaped_json_pointer_token() {
    let p = projected(EventKind::ApprovalRequested {
        request_id: "approval/~one".into(),
        action: Some("file_write".into()),
        detail: None,
        locator: None,
    });

    assert_eq!(
        p.events[1].body["delta"][0]["path"],
        "/pendingApprovals/approval~1~0one"
    );
}

#[test]
fn resolving_an_approval_removes_its_pending_state_member() {
    let p = projected(EventKind::ApprovalResolved {
        request_id: "approval/~one".into(),
        allowed: true,
    });

    assert_eq!(kinds(&p), vec!["STATE_DELTA"]);
    assert!(p.ending.is_none());
    let patch = &p.events[0].body["delta"][0];
    assert_eq!(patch["op"], "remove");
    assert_eq!(patch["path"], "/pendingApprovals/approval~1~0one");
    assert_eq!(p.events[0].seq, Some(7));
}

#[test]
fn a_completed_turn_ends_the_run() {
    let p = projected(EventKind::TurnCompleted);
    assert_eq!(kinds(&p), vec!["RUN_FINISHED"]);
    assert_eq!(p.ending, Some(Ending::Finished));
    assert_eq!(p.events[0].body["outcome"]["type"], "complete");
}

#[test]
fn every_provisioning_stage_replaces_progress_with_its_layer_2_wire_value() {
    let expected = [
        "accepted",
        "runtime_requested",
        "runtime_created",
        "skills_installed",
        "attaching",
        "ready",
    ];

    for (stage, expected) in ProvisioningStage::ALL.into_iter().zip(expected) {
        let p = projected(EventKind::Progress { stage });
        assert_eq!(kinds(&p), vec!["STATE_DELTA"], "for {stage:?}");
        assert!(p.ending.is_none(), "for {stage:?}");
        assert_eq!(p.events[0].seq, Some(7), "for {stage:?}");
        assert_eq!(
            p.events[0].body,
            serde_json::json!({ "delta": [{
                "op": "replace",
                "path": "/progress",
                "value": expected,
            }] }),
            "for {stage:?}"
        );
    }
}

#[test]
fn every_session_status_keeps_the_layer_2_wire_spelling() {
    for (status, expected) in [
        (SessionStatus::Provisioning, "provisioning"),
        (SessionStatus::Ready, "ready"),
        (SessionStatus::Running, "running"),
        (SessionStatus::Rotating, "rotating"),
        (SessionStatus::Idle, "idle"),
        (SessionStatus::Terminated, "terminated"),
    ] {
        assert_eq!(
            serde_json::to_value(status).expect("a session status serializes"),
            serde_json::json!(expected),
            "for {status:?}"
        );
    }
}

#[test]
fn every_non_ending_status_change_replaces_status_once() {
    for (status, expected) in [
        (SessionStatus::Provisioning, "provisioning"),
        (SessionStatus::Ready, "ready"),
        (SessionStatus::Running, "running"),
        (SessionStatus::Idle, "idle"),
    ] {
        let p = projected(EventKind::StatusChanged {
            status,
            stop_reason: (status == SessionStatus::Idle).then_some(StopReason::RequiresAction),
            detail: None,
            capability: None,
        });

        assert_eq!(kinds(&p), vec!["STATE_DELTA"], "for {status:?}");
        assert!(p.ending.is_none(), "for {status:?}");
        assert_eq!(p.events[0].seq, Some(7), "for {status:?}");
        assert_eq!(
            p.events[0].body,
            serde_json::json!({ "delta": [{
                "op": "replace",
                "path": "/status",
                "value": expected,
            }] }),
            "for {status:?}"
        );
    }
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
        assert!(
            p.events.iter().all(|event| event.kind != "STATE_DELTA"),
            "an ending idle transition must not also replace /status for {reason:?}"
        );
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
        moving.events[0].body["cma:returns"], true,
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
        failed.events[0].body["cma:returns"], false,
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
    assert_eq!(kinds(&p), vec!["STATE_DELTA"]);
    assert!(p.ending.is_none());
    assert_eq!(
        p.events[0].body["delta"][0],
        serde_json::json!({ "op": "replace", "path": "/status", "value": "idle" })
    );
}

#[test]
fn a_lost_stretch_of_conversation_is_shown_as_a_boundary() {
    // Rather than a stream that silently skips, leaving its reader to notice.
    let p = projected(EventKind::ConversationGap { frames: 17 });
    assert_eq!(kinds(&p), vec!["STATE_DELTA"]);
    assert_eq!(p.events[0].body["delta"][0]["value"]["frames"], 17);
}

#[test]
fn a_lost_runtime_is_appended_to_its_own_state_path() {
    let mut event = logged(
        31,
        EventKind::RuntimeLost {
            runtime_id: "runtime-gone".into(),
            cause: Some(layer2_core::RuntimeLossCause::OutOfMemory),
        },
    );
    event.appended_at = "2026-08-23T04:12:00Z".into();

    let p = project(&event, "run_1", "sesn_1");

    assert_eq!(kinds(&p), vec!["STATE_DELTA"]);
    assert_eq!(
        p.events[0].body,
        serde_json::json!({ "delta": [{
            "op": "add",
            "path": "/runtimeLosses/-",
            "value": {
                "runtime": "runtime-gone",
                "cause": "out_of_memory",
                "noticedAt": "2026-08-23T04:12:00Z",
            },
        }] })
    );
}

#[test]
fn a_lost_runtime_with_no_cause_says_so_rather_than_omitting_it() {
    let p = projected(EventKind::RuntimeLost {
        runtime_id: "runtime-silent".into(),
        cause: None,
    });
    let value = &p.events[0].body["delta"][0]["value"];

    assert!(value.get("cause").is_some(), "got {value}");
    assert!(value["cause"].is_null(), "got {value}");
}

#[tokio::test]
async fn a_lost_runtime_carries_a_cursor() {
    let p = project(
        &logged(
            43,
            EventKind::RuntimeLost {
                runtime_id: "runtime-cursored".into(),
                cause: Some(layer2_core::RuntimeLossCause::Displaced),
            },
        ),
        "run_1",
        "sesn_1",
    );

    assert_eq!(p.events[0].seq, Some(43));
    let wire = on_the_wire(&p.events[0]).await;
    assert!(wire.contains("id: 43"), "{wire:?}");
}

#[test]
fn a_lost_runtime_does_not_end_the_run() {
    let p = projected(EventKind::RuntimeLost {
        runtime_id: "runtime-replaced".into(),
        cause: Some(layer2_core::RuntimeLossCause::Failed),
    });

    assert!(p.ending.is_none());
}

#[test]
fn an_open_pressure_episode_is_durable_state_and_does_not_end_the_run() {
    let mut event = logged(
        51,
        EventKind::MemoryPressureOpened {
            used_bytes: 892_211_200,
            limit_bytes: 1_073_741_824,
            occupancy_permille: 830,
        },
    );
    event.appended_at = "2026-08-23T09:14:06Z".into();

    let p = project(&event, "run_1", "sesn_1");

    assert_eq!(kinds(&p), vec!["STATE_DELTA"]);
    assert_eq!(p.events[0].seq, Some(51), "the durable edge has a cursor");
    assert_eq!(
        p.events[0].body,
        serde_json::json!({ "delta": [{
            "op": "replace",
            "path": "/memoryPressure",
            "value": {
                "usedBytes": 892_211_200_u64,
                "limitBytes": 1_073_741_824_u64,
                "occupancyPermille": 830,
                "since": "2026-08-23T09:14:06Z",
            },
        }] })
    );
    assert!(p.ending.is_none(), "pressure is not a run failure");
}

#[test]
fn a_closed_pressure_episode_clears_the_path_with_a_value() {
    let p = projected(EventKind::MemoryPressureClosed {
        peak_bytes: 966_367_641,
        limit_bytes: 1_073_741_824,
        peak_permille: 900,
        duration_seconds: 180,
    });
    let patch = &p.events[0].body["delta"][0];

    assert_eq!(kinds(&p), vec!["STATE_DELTA"]);
    assert_eq!(patch["op"], "replace");
    assert_ne!(patch["op"], "remove");
    assert_eq!(patch["path"], "/memoryPressure");
    assert!(patch["value"].is_null());
    assert!(p.ending.is_none());
}

#[test]
fn an_open_pressure_episode_without_an_old_append_time_carries_an_absent_since() {
    let p = projected(EventKind::MemoryPressureOpened {
        used_bytes: 800,
        limit_bytes: 1_000,
        occupancy_permille: 800,
    });
    assert!(p.events[0].body["delta"][0]["value"]["since"].is_null());
}

#[test]
fn what_this_interface_cannot_carry_becomes_nothing() {
    // The core's log is the union of what every interface needs. An authority
    // marker, resize, attachment, or screen has no honest shape here, and
    // inventing one would put events in a client's stream that mean nothing.
    for kind in [
        EventKind::RuntimeHandoverStarted,
        EventKind::RuntimeIncarnationStarted,
        EventKind::RuntimeStarted {
            generation: std::num::NonZeroU64::new(3).expect("the sample generation is positive"),
        },
        EventKind::TerminalAttached,
        EventKind::TerminalDetached,
        EventKind::TerminalResized {
            columns: 120,
            rows: 40,
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
    assert_eq!(p.events[0].seq, Some(7));
    let rendered = serde_json::to_value(&p.events[0]).expect("serializes");
    assert!(rendered.get("seq").is_none(), "got {rendered}");
    assert_eq!(rendered["type"], "RUN_STARTED");
}

#[test]
fn the_commands_a_kernel_offers_replay_onto_the_live_lane_s_own_path() {
    // The kernel announces these once, as the conversation opens — before any
    // client has subscribed. Replay is the only way a client ever sees them,
    // and it must land on the same state path the live lane writes, or a
    // reader who was already watching and one who reloaded hold two different
    // lists under two different names.
    let p = projected(EventKind::AvailableCommandsSet {
        commands: vec![
            layer2_core::event::AvailableCommand {
                name: "review".into(),
                description: "Review the working tree".into(),
            },
            layer2_core::event::AvailableCommand {
                name: "compact".into(),
                description: "Summarise the conversation so far".into(),
            },
        ],
    });

    assert_eq!(kinds(&p), vec!["STATE_DELTA"]);
    let delta = &p.events[0].body["delta"][0];
    assert_eq!(delta["op"], "replace");
    assert_eq!(delta["path"], "/availableCommands");
    assert_eq!(delta["value"][0]["name"], "review");
    assert_eq!(delta["value"][0]["description"], "Review the working tree");
    assert_eq!(delta["value"][1]["name"], "compact");
    assert!(p.ending.is_none());
}

#[test]
fn a_kernel_that_offers_nothing_replays_an_empty_list_rather_than_no_patch() {
    // "This kernel has no commands" and "nobody has said yet" are different
    // facts, and a client renders them differently. Skipping the patch when
    // the list is empty would leave a stale list standing.
    let p = projected(EventKind::AvailableCommandsSet { commands: vec![] });

    assert_eq!(kinds(&p), vec!["STATE_DELTA"]);
    let delta = &p.events[0].body["delta"][0];
    assert_eq!(delta["path"], "/availableCommands");
    assert_eq!(
        delta["value"].as_array().map(Vec::len),
        Some(0),
        "got {}",
        p.events[0].body
    );
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

/// The bytes a frame actually becomes on the wire.
///
/// Rendered through axum's own response machinery rather than inspected as a
/// type, because the defect these guard was invisible at the type level: the
/// frame was correct in every way except one field that only exists once it is
/// serialised, and every test in this file passed while the browser received
/// nothing at all.
async fn on_the_wire(event: &AguiEvent) -> String {
    use axum::response::IntoResponse;
    let frame = super::sse_frame(event);
    let response = axum::response::sse::Sse::new(futures_util::stream::once(async move {
        Ok::<_, std::convert::Infallible>(frame)
    }))
    .into_response();
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("the body is small");
    String::from_utf8(body.to_vec()).expect("frames are text")
}

#[tokio::test]
async fn a_frame_carries_no_event_name() {
    // The reason this matters is not style. `EventSource` has no wildcard
    // listener: a frame carrying `event: X` is delivered only to a listener
    // registered for exactly `X`, and never to `onmessage`. So a named frame
    // is unreceivable by any client that has not heard of that kind — which
    // this service's additive event set makes routine, and which
    // `docs/API_AGUI.md` explicitly requires to keep working.
    //
    // While the frames were named, the deployed chat surface received nothing
    // whatsoever. Not the liveness lane — everything.
    let projected = project(
        &logged(
            4,
            EventKind::MessageCommitted {
                content: "hi".into(),
            },
        ),
        "run_1",
        "sesn_1",
    );
    let wire = on_the_wire(&projected.events[0]).await;

    assert!(
        !wire.contains("event:"),
        "a named frame is unreachable for a client that does not know the name: {wire:?}"
    );
    assert!(
        wire.contains("\"type\":\"TEXT_MESSAGE_START\""),
        "the kind travels in the body, which is what a client dispatches on: {wire:?}"
    );
}

#[tokio::test]
async fn a_durable_frame_carries_its_sequence_as_the_sse_id() {
    // The cursor a client resumes on. It is deliberately not in the body, so
    // this is the only place it can be, and a client reading it from anywhere
    // else gets nothing.
    let projected = project(&logged(41, EventKind::TurnStarted), "run_1", "sesn_1");
    let wire = on_the_wire(&projected.events[0]).await;

    assert!(wire.contains("id: 41"), "{wire:?}");
}

#[tokio::test]
async fn a_leading_durable_projection_frame_carries_no_sse_id() {
    let projected = project(
        &logged(
            41,
            EventKind::MessageCommitted {
                content: "the complete answer".into(),
            },
        ),
        "run_1",
        "sesn_1",
    );
    let wire = on_the_wire(&projected.events[0]).await;

    assert!(!wire.contains("id:"), "{wire:?}");
    assert!(
        !wire.contains("\"cma:live\":true"),
        "a cursorless projection frame is still durable history: {wire:?}"
    );
    assert!(wire.contains("\"type\":\"TEXT_MESSAGE_START\""), "{wire:?}");
}

#[tokio::test]
async fn a_liveness_frame_carries_no_sse_id_at_all() {
    // Given one, it becomes a cursor: a client reconnecting on it asks the log
    // to continue from a position the log has never held, and everything
    // between there and the real position is skipped.
    let live = crate::agui::live::project_live(
        &layer2_core::LiveFrame::MessageDelta("half a word".into()),
        "run_1",
        "sesn_1",
    )
    .expect("a message being typed has a shape");
    let wire = on_the_wire(&live).await;

    assert!(!wire.contains("id:"), "{wire:?}");
    assert!(wire.contains("\"cma:live\":true"), "{wire:?}");
}
