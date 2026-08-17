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
    let tickets = TerminalTickets::new();
    let (admitted, claim) = admit(
        &tickets,
        &SessionId("sesn_1".into()),
        None,
        Some(AuthenticatedCaller(caller("subject-a"))),
    )
    .await
    .expect("an authenticated caller is admitted");
    assert_eq!(admitted.subject, SubjectId("subject-a".into()));
    assert_eq!(claim, AttachmentClaim::Exclusive);
}

#[tokio::test]
async fn a_bearer_client_attaches_by_redeeming_its_ticket() {
    // The path a product frontend takes, because its browser cannot put a
    // credential on the upgrade at all.
    let tickets = TerminalTickets::new();
    let id = SessionId("sesn_1".into());
    let issued = tickets
        .issue(&caller("subject-a"), &id, AttachmentClaim::Takeover)
        .await;

    let (admitted, claim) = admit(&tickets, &id, Some(issued.value), None)
        .await
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
    let tickets = TerminalTickets::new();
    assert!(admit(&tickets, &SessionId("sesn_1".into()), None, None)
        .await
        .is_none());
}

#[tokio::test]
async fn a_spent_ticket_does_not_fall_back_to_the_cookie() {
    // Otherwise single use would hold only for clients that had no cookie,
    // and a replayed ticket would succeed on a same-origin page.
    let tickets = TerminalTickets::new();
    let id = SessionId("sesn_1".into());
    let issued = tickets
        .issue(&caller("subject-a"), &id, AttachmentClaim::Exclusive)
        .await;
    admit(&tickets, &id, Some(issued.value.clone()), None)
        .await
        .expect("the first redemption works");

    assert!(admit(
        &tickets,
        &id,
        Some(issued.value),
        Some(AuthenticatedCaller(caller("subject-a")))
    )
    .await
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
}

#[test]
fn ready_frame_uses_the_documented_field_names() {
    let msg = ServerControl::Ready {
        cols: 120,
        rows: 40,
        snapshot: true,
        conforms_to: CONFORMS_TO,
        limits: Limits {
            scrollback_lines: SCROLLBACK_LINES,
            max_frame_bytes: MAX_FRAME_BYTES,
        },
    };
    let json = serde_json::to_value(&msg).expect("serializes");
    assert_eq!(json["t"], "ready");
    assert_eq!(json["cols"], 120);
    assert_eq!(json["conformsTo"][0], CONFORMS_TO[0]);
    assert_eq!(json["limits"]["scrollbackLines"], 2000);
    assert_eq!(json["limits"]["maxFrameBytes"], 262_144);
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
