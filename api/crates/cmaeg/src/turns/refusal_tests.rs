use super::refusal::{classify_start_failure, ProviderCall};
use crate::providers::ProviderError;
use cmaeg_core::problem::ProblemCode;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn logged(error: ProviderError) -> String {
    let logs = Logs::default();
    let writer = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        classify_start_failure("nyxbot", ProviderCall::Responses.failed(error));
    });
    let result = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert_eq!(result.lines().count(), 1);
    assert!(result.contains("WARN"));
    result
}

#[test]
fn rejected_codes_are_logged_only_from_the_closed_vocabulary() {
    for code in [
        "media_source_refused",
        "partition_agent_missing",
        "partition_agent_unavailable",
        "agent_command_admission_conflict",
        "agent_command_key_conflict",
        "binding_not_ready",
        "binding_resources_unavailable",
        "command_authority_required",
        "response_forbidden",
        "response_not_found",
        "model_mismatch",
    ] {
        let line = logged(ProviderError::Rejected {
            status: 403,
            code: code.into(),
        });
        assert!(
            line.contains(&format!("upstream_code=\"{code}\"")),
            "{line}"
        );
        assert!(line.contains("upstream_status=403"), "{line}");
    }
}

#[test]
fn unknown_codes_are_redacted_from_logs_and_public_metadata() {
    for code in [
        "a",
        "0",
        "_-.",
        "quota.limit-guest-42_denied",
        "private-account-identifier_42",
        "media_source_refused.private-value",
        &"a".repeat(64),
        "",
        &"a".repeat(65),
        "DENIED",
        "denied guest",
        "denied\tguest",
        "denied\r\nWARN forged",
        "denied\0",
        "denied\u{7f}",
        "é",
        "拒绝",
        "denied\" provider=\"forged",
        "<script>alert(1)</script>",
        "denied\\nforged",
    ] {
        let line = logged(ProviderError::Rejected {
            status: 403,
            code: code.into(),
        });
        assert!(line.contains("upstream_code=\"unrecognized\""), "{line}");
        assert!(
            line.contains("gateway_detail=\"provider_rejected\""),
            "{line}"
        );
        assert!(line.contains("gateway_status=502"), "{line}");
        for forbidden in ["forged", "denied", "script"] {
            assert!(!line.contains(forbidden), "{line}");
        }
        let refusal = classify_start_failure(
            "nyxbot",
            ProviderCall::Responses.failed(ProviderError::Rejected {
                status: 403,
                code: code.into(),
            }),
        );
        let body = cmaeg_core::problem::Problem::new(refusal.code, refusal.status, refusal.detail)
            .with_upstream(refusal.upstream.unwrap());
        let json = serde_json::to_value(body).unwrap();
        assert_eq!(json["code"], "provider_rejected");
        assert_eq!(json["status"], 502);
        assert_eq!(
            json["upstream"],
            serde_json::json!({
                "operation": "responses", "status": 403, "code": "unrecognized"
            })
        );
        assert!(!line.contains("private-account-identifier_42"));
        assert!(!line.contains("media_source_refused.private-value"));
    }
}

#[test]
fn other_errors_log_only_static_variant_labels() {
    for (error, label) in [
        (
            ProviderError::UnknownProvider("private diagnostic".into()),
            "unknown_provider",
        ),
        (
            ProviderError::InvalidRequest("private diagnostic".into()),
            "invalid_request",
        ),
        (
            ProviderError::Busy {
                retry_after: Some(std::time::Duration::from_secs(12345)),
            },
            "busy",
        ),
        (ProviderError::Capacity, "capacity"),
        (ProviderError::ConversationMissing, "conversation_missing"),
        (
            ProviderError::Unavailable("private diagnostic".into()),
            "unavailable",
        ),
        (
            ProviderError::ResponseMalformed("private diagnostic".into()),
            "response_malformed",
        ),
        (ProviderError::Sse("private diagnostic".into()), "sse"),
    ] {
        let line = logged(error);
        assert!(
            line.contains(&format!("provider_error=\"{label}\"")),
            "{line}"
        );
        for forbidden in ["private", "diagnostic", "12345", "retry_after", "upstream_"] {
            assert!(!line.contains(forbidden), "{line}");
        }
    }
}

#[test]
fn rejected_provider_detail_is_carried_as_a_bounded_relay_contract() {
    let refusal = classify_start_failure(
        "nyxbot",
        ProviderCall::Responses.failed(ProviderError::Rejected {
            status: 403,
            code: "media_source_refused".into(),
        }),
    );
    assert_eq!(refusal.code, ProblemCode::ProviderRejected);
    let upstream = refusal.upstream.expect("provider detail");
    assert_eq!(upstream.operation, "responses");
    assert_eq!(upstream.status, 403);
    assert_eq!(upstream.code, "media_source_refused");
    let body = cmaeg_core::problem::Problem::new(refusal.code, refusal.status, refusal.detail)
        .with_upstream(upstream);
    let json = serde_json::to_value(body).unwrap();
    assert_eq!(json["code"], "provider_rejected");
    assert_eq!(json["upstream"]["operation"], "responses");
    assert_eq!(json["upstream"]["status"], 403);
    assert_eq!(json["upstream"]["code"], "media_source_refused");
}

#[test]
fn malformed_provider_code_is_normalized_before_relay_serialization() {
    let refusal = classify_start_failure(
        "nyxbot",
        ProviderCall::Conversation.failed(ProviderError::Rejected {
            status: 403,
            code: "secret detail with spaces".into(),
        }),
    );
    let upstream = refusal.upstream.expect("provider detail");
    assert_eq!(upstream.operation, "conversation");
    assert_eq!(upstream.code, "unrecognized");
    assert!(!serde_json::to_string(&upstream)
        .unwrap()
        .contains("secret detail"));
}
