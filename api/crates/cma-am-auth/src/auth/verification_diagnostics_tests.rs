use std::{
    io::Write,
    sync::{Arc, Mutex},
};

use layer2_core::diagnostic_context::DiagnosticContext;
use tracing::Metadata;
use tracing_subscriber::{filter::filter_fn, fmt, layer::SubscriberExt, Layer};

use super::*;
use crate::provider::verification_error::ProviderVerificationError as Error;

const PRIVATE_CALLER: &str = "synthetic-private-provider-caller";
const PRIVATE_ERROR: &str = "synthetic-private-provider-error";
const SAFE_REQUEST: &str = "synthetic-provider-request-hash";
const EVENT: &str = "identity_verification_failure";

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

impl Output {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

fn hides_identity_root(metadata: &Metadata<'_>) -> bool {
    !metadata.is_span() || metadata.name() != "identity_verification"
}

fn emit() {
    let caller = tracing::info_span!("provider_caller", private = PRIVATE_CALLER);
    let _entered = caller.enter();
    DiagnosticContext {
        request_hash: SAFE_REQUEST.into(),
        ..DiagnosticContext::default()
    }
    .sync_scope(|| {
        record(
            Endpoint::Introspection,
            Stage::Status,
            Reason::HttpStatus,
            Some(503),
            &Error::Unavailable(PRIVATE_ERROR.into()),
        );
    });
    tracing::info!("ordinary diagnostic control");
}

#[test]
fn identity_diagnostic_requires_the_exact_root_in_its_dispatcher() {
    // A live second dispatcher can leave the shared callsite interest enabled.
    let _other = tracing::Dispatch::new(fmt().finish());
    let output = Output::default();
    let json = fmt::layer()
        .json()
        .event_format(crate::diagnostic_format::json())
        .with_writer(output.clone())
        .with_filter(filter_fn(hides_identity_root));
    tracing::subscriber::with_default(tracing_subscriber::registry().with(json), emit);

    let records = output.text();
    assert!(!records.contains(EVENT));
    assert!(!records.contains(PRIVATE_ERROR));
    assert!(records.contains(PRIVATE_CALLER));
    assert!(records.contains("ordinary diagnostic control"));
}

#[test]
fn identity_diagnostic_uses_the_exact_root_visible_to_each_output_layer() {
    for hide_json_root in [false, true] {
        let json_output = Output::default();
        let text_output = Output::default();
        let json = fmt::layer()
            .json()
            .event_format(crate::diagnostic_format::json())
            .with_writer(json_output.clone())
            .with_filter(filter_fn(move |metadata| {
                !hide_json_root || hides_identity_root(metadata)
            }));
        let text = fmt::layer()
            .event_format(crate::diagnostic_format::text())
            .with_ansi(false)
            .with_writer(text_output.clone());
        tracing::subscriber::with_default(
            tracing_subscriber::registry().with(json).with(text),
            emit,
        );

        let records = [
            (json_output.text(), !hide_json_root),
            (text_output.text(), true),
        ];
        for (records, visible) in &records {
            let events: Vec<_> = records
                .lines()
                .filter(|line| line.contains(EVENT))
                .collect();
            assert_eq!(events.len(), usize::from(*visible), "{records}");
            for event in events {
                assert!(event.contains(SAFE_REQUEST));
                assert!(!event.contains(PRIVATE_CALLER));
                assert!(!event.contains(PRIVATE_ERROR));
            }
            assert!(records.lines().any(|line| {
                line.contains("ordinary diagnostic control") && line.contains(PRIVATE_CALLER)
            }));
        }
        if !hide_json_root {
            let encoded = &records[0].0;
            let diagnostic: serde_json::Value =
                serde_json::from_str(encoded.lines().find(|line| line.contains(EVENT)).unwrap())
                    .unwrap();
            assert_eq!(diagnostic["fields"]["endpoint"], "introspection");
            assert_eq!(diagnostic["fields"]["stage"], "status");
            assert_eq!(diagnostic["fields"]["reason"], "http_status");
            assert_eq!(diagnostic["fields"]["status"], 503);
            assert_eq!(diagnostic["fields"]["disposition"], "unavailable");
            assert!(diagnostic.get("span").is_none());
            assert!(diagnostic.get("spans").is_none());
        }
    }
}
