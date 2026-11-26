use std::{
    io::Write,
    sync::{Arc, Mutex},
};

use layer2_core::diagnostic_context::DiagnosticContext;
use tracing_subscriber::{fmt, layer::SubscriberExt};

const SAFE_REQUEST: &str = "synthetic-proxy-request-hash";
const PRIVATE_CALLER: &str = "synthetic-private-proxy-caller";

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> fmt::MakeWriter<'a> for Output {
    type Writer = Self;

    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

fn captured(emit: impl FnOnce()) -> Vec<serde_json::Value> {
    let output = Output::default();
    let json = fmt::layer()
        .json()
        .event_format(crate::diagnostic_format::json())
        .with_writer(output.clone());
    tracing::subscriber::with_default(tracing_subscriber::registry().with(json), || {
        let caller = tracing::info_span!("proxy_caller", private = PRIVATE_CALLER);
        let _entered = caller.enter();
        DiagnosticContext {
            request_hash: SAFE_REQUEST.into(),
            ..DiagnosticContext::default()
        }
        .sync_scope(emit);
    });
    let text = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert!(
        !text.contains(PRIVATE_CALLER),
        "the event must not carry its caller's span"
    );
    text.lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn field<'a>(record: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    record
        .get("fields")
        .and_then(|fields| fields.get(name))
        .or_else(|| record.get(name))
        .and_then(serde_json::Value::as_str)
}

#[test]
fn a_disabled_proxy_lane_records_why_an_assertion_was_ignored() {
    let records = captured(super::note_disabled);
    assert_eq!(records.len(), 1, "{records:?}");
    let record = &records[0];
    assert_eq!(
        field(record, "event"),
        Some("identity_verification_failure")
    );
    assert_eq!(field(record, "endpoint"), Some("identity_proxy"));
    assert_eq!(field(record, "stage"), Some("configuration"));
    assert_eq!(field(record, "reason"), Some("proxy_disabled"));
    assert_eq!(field(record, "disposition"), Some("rejected"));
    assert_eq!(field(record, "request_hash"), Some(SAFE_REQUEST));
}

#[test]
fn a_rejected_assertion_records_only_its_fixed_reason() {
    let records = captured(|| super::note_unaccepted("assertion", "token_destination"));
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(field(&records[0], "stage"), Some("assertion"));
    assert_eq!(field(&records[0], "reason"), Some("token_destination"));
}
