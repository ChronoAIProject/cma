//! Public rows validate names and never expose arbitrary provider error strings.
use cmaeg::{
    http::consumer_admission::ConsumerAdmission,
    ingress::launch::{LaunchOutcome, LaunchRefusal},
    turns::fanout::ConsumerLaunch,
};
use cmaeg_core::problem::{Problem, ProblemCode};
use serde_json::{json, Value};

fn launch(name: &str, outcome: LaunchOutcome) -> ConsumerLaunch {
    ConsumerLaunch {
        id: cmaeg_core::ids::ConsumerId::default_consumer(),
        name: name.into(),
        conversation_id: "conversation-fixture".parse().unwrap(),
        outcome,
    }
}
#[test]
fn admitted_rows_require_identity_and_omit_failure_metadata() {
    for (outcome, status) in [
        (
            LaunchOutcome::Accepted {
                response_id: "response-fixture".into(),
            },
            "accepted",
        ),
        (LaunchOutcome::Deferred, "deferred"),
    ] {
        let row = ConsumerAdmission::try_from(launch("main", outcome)).unwrap();
        assert_eq!(
            serde_json::to_value(row).unwrap(),
            json!({"consumer_id":"default","name":"main","conversation_id":"conversation-fixture","status":status})
        );
    }
    assert!(ConsumerAdmission::try_from(launch("invalid name", LaunchOutcome::Deferred)).is_err());
}
#[test]
fn refused_rows_sanitize_provider_codes() {
    for (code, expected) in [
        ("binding_disabled", Some("binding_disabled")),
        ("Private Provider Response", None),
        ("", None),
    ] {
        let row = ConsumerAdmission::try_from(launch(
            "main",
            LaunchOutcome::Refused(LaunchRefusal {
                problem: Problem::new(ProblemCode::ProviderUnavailable, 503, "private detail"),
                code: "provider_rejected",
                provider_code: Some(code.into()),
            }),
        ))
        .unwrap();
        let value: Value = serde_json::to_value(row).unwrap();
        assert_eq!(value["status"], "refused");
        assert_eq!(value["code"], "provider_rejected");
        assert_eq!(value.get("provider_code").and_then(Value::as_str), expected);
        assert!(!value.to_string().contains("private detail"));
    }
}
