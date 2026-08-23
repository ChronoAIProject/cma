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

fn empty_resource_reading() -> ResourceReading {
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
fn a_resource_body_carries_all_fourteen_fields_as_explicit_nulls() {
    let rendered = serde_json::to_value(ResourceStatusView::from(ResourceStatus::Held(
        layer2_core::HeldReading {
            reading: empty_resource_reading(),
            observed_at: "2026-08-23T09:14:06Z".into(),
        },
    )))
    .expect("serializes");
    let reading = rendered["reading"].as_object().expect("a reading object");

    assert_eq!(reading.len(), 14, "got {reading:?}");
    assert!(reading.values().all(serde_json::Value::is_null));
    assert_eq!(rendered["observed_at"], "2026-08-23T09:14:06Z");
    assert!(rendered["reason"].is_null());
}

#[test]
fn awaiting_and_unreported_are_two_different_complete_answers() {
    let awaiting = serde_json::to_value(ResourceStatusView::from(ResourceStatus::Awaiting))
        .expect("serializes");
    let unreported = serde_json::to_value(ResourceStatusView::from(ResourceStatus::Unreported))
        .expect("serializes");

    assert_eq!(
        awaiting,
        serde_json::json!({
            "reading": null,
            "observed_at": null,
            "reason": "no_reading_yet",
        })
    );
    assert_eq!(
        unreported,
        serde_json::json!({
            "reading": null,
            "observed_at": null,
            "reason": "runtime_does_not_report",
        })
    );
}

#[test]
fn a_reported_reading_with_no_ceiling_keeps_that_absence() {
    let mut reading = empty_resource_reading();
    reading.memory_bytes_used = Some(892_211_200);
    let rendered = serde_json::to_value(ResourceStatusView::from(ResourceStatus::Held(
        layer2_core::HeldReading {
            reading,
            observed_at: "2026-08-23T09:14:06Z".into(),
        },
    )))
    .expect("serializes");

    assert_eq!(rendered["reading"]["memory_bytes_used"], 892_211_200_u64);
    assert!(rendered["reading"]["memory_bytes_limit"].is_null());
    assert!(rendered["reason"].is_null());
}

#[test]
fn spend_crosses_as_exact_decimal_strings_with_server_rendered_display() {
    let rendered = serde_json::to_value(SpendView::from(Spend {
        priced: true,
        rate_card: Some(layer2_core::RateCardId("rate_exact".into())),
        micros_per_hour: Some(45_000),
        accrued_micros: Some(45_000),
        billable_seconds: Some(3_600),
        accruing: false,
        unbounded_intervals: 0,
    }))
    .expect("serializes");

    assert_eq!(
        rendered,
        serde_json::json!({
            "priced": true,
            "rate_card": "rate_exact",
            "currency": "USD",
            "micros_per_hour": "45000",
            "accrued_micros": "45000",
            "display": "$0.045",
            "billable_seconds": 3600,
            "accruing": false,
            "unbounded_intervals": 0
        })
    );
}

#[test]
fn unpriced_spend_omits_every_figure_instead_of_rendering_zero() {
    let rendered = serde_json::to_value(SpendView::from(Spend {
        priced: false,
        rate_card: None,
        micros_per_hour: None,
        accrued_micros: None,
        billable_seconds: None,
        accruing: false,
        unbounded_intervals: 0,
    }))
    .expect("serializes");

    assert_eq!(
        rendered,
        serde_json::json!({
            "priced": false,
            "accruing": false,
            "unbounded_intervals": 0
        })
    );
    assert!(!rendered.to_string().contains("$0.00"));
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
