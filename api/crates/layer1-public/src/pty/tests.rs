//! Tests for the terminal interface.
//!
//! Kept beside the module rather than inside it so that file stays within
//! the project's size limit while the behaviour remains covered.

use super::*;

#[test]
fn projection_matches_what_the_protocol_can_express() {
    let ops = PtyInterface.operations();
    assert!(ops.contains(&Operation::SendInput));
    assert!(ops.contains(&Operation::ResizeTerminal));
    assert!(ops.contains(&Operation::ScreenSnapshot));
    // Sessions are created over a request/response interface first; this
    // one only ever attaches to an existing session.
    assert!(!ops.contains(&Operation::CreateSession));
    assert!(!ops.contains(&Operation::GetUsage));
    assert!(!ops.contains(&Operation::Steer));
}

use layer2_core::{Authorization, Capability, SubjectId};

fn caller(subject: &str) -> Caller {
    Caller::new(
        SubjectId(subject.to_string()),
        Authorization::from_capabilities([Capability::UseOwnSessions]),
    )
}

#[tokio::test]
async fn a_cookie_client_attaches_as_itself_and_exclusively() {
    // The path our own dashboard takes, unchanged by any of this: the
    // browser sends its cookie, middleware has already resolved who that
    // is, and no ticket is involved.
    let tickets = TerminalTickets::in_memory();
    let (admitted, claim) = admit(
        &tickets,
        &SessionId("sesn_1".into()),
        None,
        Some(AuthenticatedCaller(caller("subject-a"))),
    )
    .await
    .expect("ticket store answers")
    .expect("an authenticated caller is admitted");
    assert_eq!(admitted.subject, SubjectId("subject-a".into()));
    assert_eq!(claim, AttachmentClaim::Exclusive);
}

#[tokio::test]
async fn a_bearer_client_attaches_by_redeeming_its_ticket() {
    // The path a product frontend takes, because its browser cannot put a
    // credential on the upgrade at all.
    let tickets = TerminalTickets::in_memory();
    let id = SessionId("sesn_1".into());
    let issued = tickets
        .issue(&caller("subject-a"), &id, AttachmentClaim::Takeover)
        .await
        .expect("ticket is stored");

    let (admitted, claim) = admit(&tickets, &id, Some(issued.value), None)
        .await
        .expect("ticket store answers")
        .expect("a live ticket is admitted");
    assert_eq!(admitted.subject, SubjectId("subject-a".into()));
    assert_eq!(
        claim,
        AttachmentClaim::Takeover,
        "the claim is decided where the caller was authenticated, not at the upgrade"
    );
}

#[tokio::test]
async fn an_upgrade_with_neither_a_ticket_nor_a_cookie_is_refused() {
    let tickets = TerminalTickets::in_memory();
    assert!(admit(&tickets, &SessionId("sesn_1".into()), None, None)
        .await
        .expect("ticket store answers")
        .is_none());
}

#[tokio::test]
async fn a_spent_ticket_does_not_fall_back_to_the_cookie() {
    // Otherwise single use would hold only for clients that had no cookie,
    // and a replayed ticket would succeed on a same-origin page.
    let tickets = TerminalTickets::in_memory();
    let id = SessionId("sesn_1".into());
    let issued = tickets
        .issue(&caller("subject-a"), &id, AttachmentClaim::Exclusive)
        .await
        .expect("ticket is stored");
    admit(&tickets, &id, Some(issued.value.clone()), None)
        .await
        .expect("ticket store answers")
        .expect("the first redemption works");

    assert!(admit(
        &tickets,
        &id,
        Some(issued.value),
        Some(AuthenticatedCaller(caller("subject-a")))
    )
    .await
    .expect("ticket store answers")
    .is_none());
}

#[test]
fn a_superseded_client_is_told_in_a_code_it_can_branch_on() {
    // Private range, because no registered close code says this.
    assert!(
        (4000..=4999).contains(&SUPERSEDED_CLOSE_CODE),
        "a takeover must not borrow a registered code that means something else"
    );
}

fn close_for(detached: Option<layer2_core::Detached>) -> (u16, String) {
    let frames = ending(detached);
    let Some(Message::Close(Some(close))) = frames.last() else {
        panic!("an attachment ending must finish with a close frame: {frames:?}");
    };
    (close.code, close.reason.to_string())
}

#[test]
fn each_kind_of_ending_has_its_own_code() {
    // The three ask different things of whoever is looking, so they cannot
    // share a code: one sends you to another tab, one tells you the terminal
    // is gone for good and the session is now a conversation, and one asks you
    // to come back in a moment.
    let codes = [
        close_for(Some(layer2_core::Detached::Superseded)).0,
        close_for(Some(layer2_core::Detached::PlaneChanged)).0,
        close_for(Some(layer2_core::Detached::RuntimeReplaced)).0,
    ];
    assert_eq!(
        codes
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        3,
        "a client branching on the code must be able to tell them apart"
    );
    for code in codes {
        assert!(
            (4000..=4999).contains(&code),
            "an ending must not borrow a registered code that means something else"
        );
    }
}

#[test]
fn a_session_that_left_the_terminal_plane_says_so_rather_than_going_quiet() {
    let (code, reason) = close_for(Some(layer2_core::Detached::PlaneChanged));
    assert_eq!(code, PLANE_CHANGED_CLOSE_CODE);
    assert!(
        reason.contains("conversation"),
        "the reason must name where the session went: {reason}"
    );
}

#[test]
fn an_attachment_lost_with_nothing_said_still_closes_with_a_reason() {
    // The channel can close without a word — an entry dropped for some other
    // cause. A terminal that ends silently is one whose owner reloads and
    // takes the attachment back, which is the fight this exists to prevent.
    let (code, reason) = close_for(None);
    assert_eq!(code, SUPERSEDED_CLOSE_CODE);
    assert!(!reason.is_empty());
}

#[test]
fn a_reported_process_exit_is_sent_before_a_normal_close() {
    let frames = reported_ending(Some(layer2_core::Detached::ProcessExited {
        code: Some(17),
    }));
    assert_eq!(frames.len(), 2, "got {frames:?}");

    let Message::Text(text) = &frames[0] else {
        panic!("the exit report must be a text control frame: {frames:?}");
    };
    let json: serde_json::Value = serde_json::from_str(text).expect("the exit frame is JSON");
    assert_eq!(
        json,
        serde_json::json!({ "t": "exit", "code": 17, "reason": "exited" })
    );

    let Message::Close(Some(close)) = &frames[1] else {
        panic!("the exit report must be followed by Close: {frames:?}");
    };
    assert_eq!(close.code, axum::extract::ws::close_code::NORMAL);
    assert!(close.reason.is_empty());
}

#[test]
fn a_signal_exit_carries_null_instead_of_fabricating_zero() {
    let frames = reported_ending(Some(layer2_core::Detached::ProcessExited { code: None }));
    let Message::Text(text) = &frames[0] else {
        panic!("the exit report must be text: {frames:?}");
    };
    let json: serde_json::Value = serde_json::from_str(text).expect("the exit frame is JSON");
    assert!(json["code"].is_null(), "got {json}");
    assert_eq!(json["reason"], "exited");
}

#[test]
fn an_unreported_runtime_channel_ending_emits_no_exit_or_clean_close() {
    assert!(
        reported_ending(None).is_empty(),
        "transport loss is not evidence of process exit"
    );
}

#[test]
fn a_missing_capability_has_its_own_code_here_too() {
    assert_eq!(
        error_code(&CoreError::CapabilityUnsupported("terminal".into())),
        "capability_unsupported"
    );
}

#[test]
fn unauthorized_and_missing_collapse_to_one_code() {
    assert_eq!(error_code(&CoreError::SessionNotFound), "session_not_found");
    assert_eq!(error_code(&CoreError::Forbidden), "session_not_found");
}

#[test]
fn each_remaining_failure_has_its_own_code() {
    assert_eq!(error_code(&CoreError::AlreadyAttached), "already_attached");
    assert_eq!(
        error_code(&CoreError::SessionTerminated),
        "session_terminated"
    );
    assert_eq!(
        error_code(&CoreError::RuntimeUnavailable("x".into())),
        "runtime_unavailable"
    );
    assert_eq!(
        error_code(&CoreError::PriceListNotAppendOnly("x".into())),
        "price_list_not_append_only"
    );
    assert_eq!(
        error_code(&CoreError::PriceChanged {
            quoted: "rate_old".into(),
            in_force: "rate_new".into(),
        }),
        "price_changed"
    );
}

#[test]
fn ready_frame_uses_the_documented_field_names() {
    let msg = ServerControl::Ready {
        cols: 120,
        rows: 40,
        snapshot: true,
        resources: true,
        conforms_to: CONFORMS_TO,
        limits: Limits {
            scrollback_lines: SCROLLBACK_LINES,
            max_frame_bytes: MAX_FRAME_BYTES,
        },
    };
    let json = serde_json::to_value(&msg).expect("serializes");
    assert_eq!(json["t"], "ready");
    assert_eq!(json["cols"], 120);
    assert_eq!(json["resources"], true);
    assert_eq!(json["conformsTo"][0], CONFORMS_TO[0]);
    assert_eq!(json["limits"]["scrollbackLines"], 2000);
    assert_eq!(json["limits"]["maxFrameBytes"], 262_144);
}

fn empty_reading() -> ResourceReading {
    ResourceReading {
        cpu_millis_used: None,
        cpu_millis_limit: None,
        memory_bytes_used: None,
        memory_bytes_limit: None,
        memory_bytes_peak: None,
        stall_permille: None,
        ceiling_hits: None,
        allocation_failures: None,
        kills: None,
        received_bytes: None,
        sent_bytes: None,
        received_bytes_per_second: None,
        sent_bytes_per_second: None,
        interval_ms: None,
    }
}

#[test]
fn a_resource_frame_carries_all_fourteen_camel_case_fields_as_explicit_nulls() {
    let json =
        serde_json::to_value(ServerControl::from_reading(empty_reading())).expect("serializes");
    let object = json.as_object().expect("a control object");
    let fields = [
        "cpuMillisUsed",
        "cpuMillisLimit",
        "memoryBytesUsed",
        "memoryBytesLimit",
        "memoryBytesPeak",
        "stallPermille",
        "ceilingHits",
        "allocationFailures",
        "kills",
        "receivedBytes",
        "sentBytes",
        "receivedBytesPerSecond",
        "sentBytesPerSecond",
        "intervalMs",
    ];

    assert_eq!(json["t"], "resources");
    assert_eq!(object.len(), fields.len() + 1, "got {json}");
    for field in fields {
        assert!(object.contains_key(field), "missing {field}: {json}");
        assert!(json[field].is_null(), "{field} was not null: {json}");
    }
}

#[test]
fn ready_promises_readings_only_for_a_reporting_subscription() {
    let held = layer2_core::HeldReading {
        reading: empty_reading(),
        observed_at: "2026-08-23T09:14:06Z".into(),
    };

    assert!(reports_resources(true, Ok(ResourceStatus::Awaiting)));
    assert!(reports_resources(true, Ok(ResourceStatus::Held(held))));
    assert!(!reports_resources(true, Ok(ResourceStatus::Unreported)));
    assert!(!reports_resources(true, Err(CoreError::SessionNotFound)));
    assert!(!reports_resources(false, Ok(ResourceStatus::Awaiting)));
}

#[test]
fn control_frames_parse_from_their_tag() {
    let r: ClientControl =
        serde_json::from_str(r#"{"t":"resize","cols":80,"rows":24}"#).expect("resize parses");
    assert!(matches!(r, ClientControl::Resize { cols: 80, rows: 24 }));
    let s: ClientControl =
        serde_json::from_str(r#"{"t":"signal","sig":"INT"}"#).expect("signal parses");
    assert!(matches!(s, ClientControl::Signal { .. }));
    assert!(serde_json::from_str::<ClientControl>(r#"{"t":"nope"}"#).is_err());
}

#[test]
fn every_documented_signal_name_is_supported() {
    for signal in ["INT", "TERM", "QUIT", "HUP"] {
        assert!(supported_signal(signal), "{signal} must remain accepted");
    }
}

#[test]
fn an_unknown_signal_name_is_rejected() {
    assert!(!supported_signal("XYZ"));
    assert!(!supported_signal("int"), "signal names are case-sensitive");
}
