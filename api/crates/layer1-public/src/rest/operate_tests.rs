//! What the operating routes accept, and how they refuse.
//!
//! The refusals matter as much as the successes here. Every one of these
//! routes exists on every session, and what makes that safe is that a session
//! which cannot honour one says so by name rather than accepting the request.

use axum::response::IntoResponse;

use super::*;

/// A request body as a caller would send it.
fn parse<T: for<'de> Deserialize<'de>>(body: serde_json::Value) -> Result<T, serde_json::Error> {
    serde_json::from_value(body)
}

#[test]
fn a_line_of_input_is_submitted_unless_the_caller_says_otherwise() {
    // A client sending a line means to submit it. One composing a command in
    // pieces says so, and the default is the case that is not surprising.
    let submitted: InputRequest = parse(serde_json::json!({ "text": "ls" })).expect("parses");
    assert!(submitted.newline);
    let composing: InputRequest =
        parse(serde_json::json!({ "text": "ls", "newline": false })).expect("parses");
    assert!(!composing.newline);
}

#[test]
fn raw_bytes_are_accepted_because_a_json_string_cannot_carry_a_control_sequence() {
    let request: InputRequest = parse(serde_json::json!({ "bytes_b64": "GwtC" })).expect("parses");
    assert_eq!(request.bytes_b64.as_deref(), Some("GwtC"));
}

#[test]
fn a_body_that_is_not_base64_is_refused_without_quoting_it() {
    // The one part of a request that might be a credential is its content, so
    // a refusal names the field and never the value.
    let Err(problem) = decode("not base64 at all!!") else {
        panic!("a body that is not base64 must be refused");
    };
    let rendered = format!("{:?}", problem.0);
    assert!(rendered.contains("bytes_b64"), "got {rendered}");
    assert!(
        !rendered.contains("not base64 at all"),
        "a refusal must not echo what was sent, got {rendered}"
    );
}

#[test]
fn valid_base64_decodes_to_the_bytes_a_terminal_would_receive() {
    // 0x1b 0x0b 0x42 — an escape sequence, which is exactly the case text
    // cannot express and this field exists for.
    let Ok(bytes) = decode("GwtC") else {
        panic!("valid base64 decodes");
    };
    assert_eq!(bytes, vec![0x1b, 0x0b, 0x42]);
}

#[test]
fn every_decision_a_person_can_make_crosses_the_boundary_intact() {
    // Refusing one action and abandoning the turn are different intentions,
    // and a client that meant the second and got the first watches the agent
    // try something else instead of stopping.
    for (wire, expected) in [
        ("allow", ApprovalDecision::Allow),
        ("deny", ApprovalDecision::Deny),
        ("abort", ApprovalDecision::Abort),
    ] {
        let body: ApprovalRequestBody =
            parse(serde_json::json!({ "decision": wire })).expect("parses");
        assert_eq!(
            ApprovalDecision::from(body.decision),
            expected,
            "for {wire}"
        );
    }
}

#[test]
fn a_decision_this_service_does_not_offer_is_refused_rather_than_guessed() {
    assert!(parse::<ApprovalRequestBody>(serde_json::json!({ "decision": "maybe" })).is_err());
}

#[test]
fn usage_crosses_with_the_context_window_the_core_keeps() {
    let view = UsageView::from(Usage {
        input_tokens: 412,
        output_tokens: 9,
        context_used: 1_024,
        context_window: Some(200_000),
    });
    let rendered = serde_json::to_value(&view).expect("serializes");
    assert_eq!(rendered["input_tokens"], 412);
    assert_eq!(rendered["output_tokens"], 9);
    assert_eq!(rendered["context_used"], 1_024);
    assert_eq!(rendered["context_window"], 200_000);
}

#[test]
fn usage_omits_a_context_window_the_runtime_never_stated() {
    let rendered = serde_json::to_value(UsageView::from(Usage {
        input_tokens: 412,
        output_tokens: 9,
        context_used: 1_024,
        context_window: None,
    }))
    .expect("serializes");

    assert!(rendered.get("context_window").is_none());
}

#[test]
fn a_resize_needs_both_dimensions() {
    // A geometry with one side missing is not a smaller terminal, it is an
    // unanswerable request.
    assert!(parse::<ResizeRequest>(serde_json::json!({ "cols": 120 })).is_err());
    let both: ResizeRequest =
        parse(serde_json::json!({ "cols": 120, "rows": 40 })).expect("parses");
    assert_eq!((both.cols, both.rows), (120, 40));
}

#[test]
fn a_missing_ability_is_named_rather_than_reported_as_not_found() {
    // The route exists and the request is well-formed; this session simply
    // cannot honour it. A 404 would send the caller looking for a spelling
    // mistake in a path that is perfectly correct.
    let response = Problem(CoreError::CapabilityUnsupported("steer".into())).into_response();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[test]
fn driving_a_terminal_somebody_is_holding_is_a_conflict_rather_than_a_failure() {
    // Keystrokes injected into a live attachment land in the middle of an
    // escape sequence that client is drawing. The caller is told what is
    // holding it, which is a thing they can act on.
    let response = Problem(CoreError::AlreadyAttached).into_response();
    assert_eq!(response.status(), StatusCode::CONFLICT);
}
