//! Capturing a variable deadline must still prove the public ninety-second bound.
use serde_json::Value;

pub(super) fn validate(value: &Value) {
    if let Some(result) = value.pointer("/lifecycle/result") {
        if result["code"] == "lifecycle_failed_admission_unavailable" {
            assert_eq!(result["state"], "failed");
            assert_eq!(result["continuing"], false);
            for field in [
                "operation_id",
                "started_at",
                "result_due_at",
                "retry_after_seconds",
            ] {
                assert!(result.get(field).is_none());
            }
            return;
        }
        let seconds = |key: &str| {
            layer2_core::seconds_from_rfc3339(result[key].as_str().expect("timestamp is a string"))
                .expect("valid lifecycle timestamp")
        };
        assert_eq!(
            seconds("result_due_at").checked_sub(seconds("started_at")),
            Some(90)
        );
    }
    match value {
        Value::Array(items) => items.iter().for_each(validate),
        Value::Object(fields) => fields.values().for_each(validate),
        _ => {}
    }
}

#[test]
fn variable_deadline_captures_cannot_hide_a_reset_or_extended_budget() {
    let mut body = serde_json::json!({"lifecycle":{"result":{
        "started_at":"2026-09-05T00:00:00Z", "result_due_at":"2026-09-05T00:01:30Z"
    }}});
    validate(&body);
    for due in ["2026-09-05T00:05:00Z", "2026-09-05T00:00:00Z", "unknown"] {
        body["lifecycle"]["result"]["result_due_at"] = due.into();
        assert!(std::panic::catch_unwind(|| validate(&body)).is_err());
    }
}

#[test]
fn failed_result_wire_fixture_omits_retry_after() {
    use layer2_core::sandbox::{
        SandboxLifecycleAction, SandboxLifecycleResult, SandboxLifecycleResultState,
    };
    let result = crate::rest::instances_openapi_shapes::SandboxLifecycleResultView::from(
        SandboxLifecycleResult {
            state: SandboxLifecycleResultState::Failed,
            code: Some("lifecycle_failed"),
            operation_id: Some("original".into()),
            started_at: Some("2026-11-16T00:00:00Z".into()),
            result_due_at: Some("2026-11-16T00:01:30Z".into()),
            continuing: false,
            owner_slot_released: None,
            blocked_retirement_step: None,
            actions: vec![
                SandboxLifecycleAction::Observe,
                SandboxLifecycleAction::Retire,
            ],
            retry_after_seconds: None,
        },
    );
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/lifecycle-result-failed.json"
    ))
    .unwrap();
    assert_eq!(serde_json::to_value(result).unwrap(), fixture);
}
