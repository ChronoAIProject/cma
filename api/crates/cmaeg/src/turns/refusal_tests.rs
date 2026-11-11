use super::refusal::{classify_start_failure, ProviderCall};
use crate::providers::ProviderError;
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
fn rejected_codes_are_logged_only_as_bounded_machine_tokens() {
    for code in [
        "a",
        "0",
        "_-.:",
        "quota.limit:guest-42_denied",
        &"a".repeat(64),
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
    for code in [
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
