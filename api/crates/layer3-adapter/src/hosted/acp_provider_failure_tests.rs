use super::*;
use crate::hosted::acp_turn::{scoped_turn_outcome, turn_outcome_with_policy};
use crate::{allocation_probe::AllocationProbe, ConversationUpdate, PromptIdentity};
use serde_json::{json, Value};

fn check(frame: &Value, bounded: bool, class: &str, summary: &str) {
    let Outcome::Failed {
        capability,
        message,
    } = turn_outcome_with_policy(frame, bounded)
    else {
        panic!("failure envelope")
    };
    assert_eq!(capability.as_deref(), Some(class));
    assert_eq!(message, format!("{summary} {CONTINUATION}"));
    assert!(!message.contains("SENTINEL"));
}

fn envelopes(message: &str) -> [Value; 2] {
    [
        json!({"error":{"message":message,"data":{"capability":"PRIVATE_CLASS_SENTINEL"}}}),
        json!({"result":{"stopReason":"end_turn","_meta":{"jetbrains":{"air":{"sessionFailure":{"severity":"error","title":message}}}}}}),
    ]
}

#[test]
fn recognized_provider_failures_use_only_the_bounded_transport_status() {
    for (status, class, summary) in [
        (
            408,
            "upstream_timeout",
            "The upstream model service timed out.",
        ),
        (
            504,
            "upstream_timeout",
            "The upstream model service timed out.",
        ),
        (
            522,
            "upstream_timeout",
            "The upstream model service timed out.",
        ),
        (
            524,
            "upstream_timeout",
            "The upstream model service timed out.",
        ),
        (
            500,
            "upstream_unavailable",
            "The upstream model service is temporarily unavailable.",
        ),
        (
            503,
            "upstream_unavailable",
            "The upstream model service is temporarily unavailable.",
        ),
        (
            599,
            "upstream_unavailable",
            "The upstream model service is temporarily unavailable.",
        ),
        (
            429,
            "upstream_rate_limited",
            "The upstream model service is limiting requests. Wait before sending another message.",
        ),
    ] {
        for reason in ["", " Service Unavailable", " A Timeout Occurred"] {
            for body in [
                "<html>PRIVATE_BODY_SENTINEL https://failure.invalid X-Private-Header 429</html>",
                r#"{"status":402,"error":"PRIVATE_BODY_SENTINEL"}"#,
                "PRIVATE_BODY_SENTINEL unexpected status 524:",
            ] {
                for frame in envelopes(&format!(" \tunexpected status {status}{reason}: {body}")) {
                    for bounded in [false, true] {
                        check(&frame, bounded, class, summary);
                    }
                }
            }
        }
    }
}

#[test]
fn unsupported_or_malformed_provider_failures_are_generic_despite_spoofed_classes() {
    let overlong = format!(
        "unexpected status 503 {}: PRIVATE_BODY_SENTINEL",
        "A".repeat(128)
    );
    for message in [
        "",
        "503",
        "PRIVATE_BODY_SENTINEL unexpected status 503:",
        "unexpected status 5030:",
        "unexpected status ５０３:",
        "unexpected status 503x:",
        "unexpected status 503\n:",
        "unexpected status 503 /path:",
        "unexpected status 503",
        "unexpected status 503 :",
        "unexpected status 403: unexpected status 503:",
        "unexpected status 402: {\"status\":503}",
        "unexpected status 200: PRIVATE_BODY_SENTINEL",
        "         unexpected status 503:",
        "\u{2003}unexpected status 503:",
        &overlong,
    ] {
        for frame in envelopes(message) {
            for bounded in [false, true] {
                check(
                    &frame,
                    bounded,
                    "agent_turn_failed",
                    "The Agent run failed.",
                );
            }
        }
    }
    for frame in [
        json!(null),
        json!({}),
        json!({"error":null}),
        json!({"error":{"message":17,"data":{"capability":"runtime_lost"}}}),
        json!({"result":null}),
        json!({"result":{"stopReason":"PRIVATE_BODY_SENTINEL"}}),
        json!({"result":{"stopReason":"end_turn","_meta":{"jetbrains":{"air":{"sessionFailure":{}}}}}}),
    ] {
        for bounded in [false, true] {
            check(
                &frame,
                bounded,
                "agent_turn_failed",
                "The Agent run failed.",
            );
        }
    }
}

#[test]
fn credit_identity_warning_and_success_contracts_survive_both_json_policies() {
    let credit = "unexpected status 402 Payment Required: {\"error\":\"insufficient_credits\",\"error_code\":11300,\"message\":\"PRIVATE_BODY_SENTINEL\"}";
    let prompt = PromptIdentity::new("conversation-original", 7, 3);
    for bounded in [false, true] {
        for frame in envelopes(credit) {
            let result = turn_outcome_with_policy(&frame, bounded);
            assert!(
                matches!(scoped_turn_outcome(result, true, Some(prompt.clone())), ConversationUpdate::FailedForPrompt { prompt_id, capability, message }
                if prompt_id == prompt && capability.as_deref() == Some("insufficient_credits") && message.contains("top up") && !message.contains("SENTINEL"))
            );
        }
        let success = json!({"result":{"stopReason":"end_turn","_meta":{"jetbrains":{"air":{"sessionFailure":{"severity":"warning","title":"unexpected status 503: <html>successful recovery</html>"}}}}}});
        assert!(matches!(
            turn_outcome_with_policy(success, bounded),
            Outcome::Ended(_)
        ));
        assert!(matches!(
            turn_outcome_with_policy(json!({"result":{"stopReason":"end_turn"}}), bounded),
            Outcome::Ended(_)
        ));
    }
}

#[test]
fn huge_provider_body_is_borrowed_and_never_copied_into_a_failure() {
    for bounded in [false, true] {
        for frame in envelopes(&format!(
            "unexpected status 503: {}",
            "PRIVATE_BODY_SENTINEL".repeat(200_000)
        )) {
            let mut probe = AllocationProbe::new();
            let result = probe.track(|| turn_outcome_with_policy(&frame, bounded));
            assert!(
                probe.peak() < 2048,
                "discarded body allocated {} bytes",
                probe.peak()
            );
            assert!(
                matches!(&result, Outcome::Failed { capability, message } if capability.as_deref() == Some("upstream_unavailable") && !message.contains("SENTINEL"))
            );
            drop(result);
            assert_eq!(probe.live(), 0);
        }
    }
}
