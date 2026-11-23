use super::*;
use layer2_core::{
    diagnostic_context::DiagnosticContext, sandbox::agent_io::ExternalContextObservation,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::time::Instant;
use tracing::{
    span::{Attributes, Id, Record},
    Event, Metadata, Subscriber,
};

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<BTreeMap<String, String>>>>);
struct Fields(BTreeMap<String, String>);
impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().into(), format!("{value:?}"));
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.insert(field.name().into(), value.into());
    }
}
impl Subscriber for Capture {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        if event.metadata().target() != "cma::responses::event_context" {
            return;
        }
        assert!(!event.is_contextual());
        assert!(event.parent().is_none());
        let mut fields = Fields(BTreeMap::new());
        event.record(&mut fields);
        self.0.lock().unwrap().push(fields.0);
    }
}
impl Capture {
    fn finished(&self, phase: &str) -> BTreeMap<String, String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|record| {
                record.get("phase").is_some_and(|v| v == phase) && record["outcome"] != "started"
            })
            .unwrap_or_else(|| panic!("missing completion for {phase}"))
            .clone()
    }
}

#[tokio::test]
async fn real_retained_reads_correlate_all_phases_and_preserve_legacy_and_buffered_paths() {
    for buffered in [false, true] {
        let (app, _, _, principal) =
            super::super::binding_tests::fixture_for_owner_with_router_and_context(
                layer2_core::sandbox::binding::BindingConversationState::Ready,
                false,
                "human",
                None,
                buffered,
            )
            .await;
        let response = app.clone().oneshot(request("/api/v2/responses", &principal,
            json!({"conversation":"conversation-external", "stream":true,"input":"Hello","event_context":context()})))
            .await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        drop(response);
        let capture = Capture::default();
        let _guard = tracing::subscriber::set_default(capture.clone());
        let hash = "b".repeat(64);
        for (event, status) in [
            ("event-1", StatusCode::OK),
            ("missing", StatusCode::NOT_FOUND),
        ] {
            let path = format!("/api/v1/cmaeg/bindings/binding-responses/conversations/conversation-external/events/{event}");
            let mut get = request(&path, &principal, Value::Null);
            *get.method_mut() = Method::GET;
            *get.body_mut() = Body::empty();
            let entered = Arc::new(OnceLock::<Instant>::new());
            get.extensions_mut().insert(entered.clone());
            let response = ExternalContextObservation::from_hash(hash.clone().into())
                .unwrap()
                .scope(app.clone().oneshot(get))
                .await
                .unwrap();
            assert_eq!(response.status(), status);
            assert!(entered.get().is_some());
            assert_eq!(
                capture.finished("handler")["status"],
                status.as_u16().to_string()
            );
            assert_eq!(
                capture.finished("retained_command_lookup")["outcome"],
                if status == StatusCode::OK {
                    "completed"
                } else {
                    "error"
                }
            );
        }
        let records = capture.0.lock().unwrap();
        let phases: Vec<_> = records
            .iter()
            .filter(|r| r["outcome"] == "started")
            .map(|r| r["phase"].as_str())
            .collect();
        let expected = if buffered {
            vec![
                "handler",
                "total",
                "retained_command_lookup",
                "resource_authorization",
                "final_release_fence",
                "handler",
                "total",
                "retained_command_lookup",
            ]
        } else {
            vec![
                "handler",
                "total",
                "retained_command_lookup",
                "resource_authorization",
                "retained_command_recheck",
                "final_release_fence",
                "handler",
                "total",
                "retained_command_lookup",
            ]
        };
        assert_eq!(phases, expected);
        assert!(records.iter().all(|r| r["attempt_hash"] == hash));
        let rendered = format!("{records:?}");
        for private in [
            "binding-responses",
            "conversation-external",
            "event-1",
            "local-fixture",
            "sealed-reference",
            "Hello",
            "owner-a",
        ] {
            assert!(!rendered.contains(private));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn setup_diagnostics_distinguish_absence_refusal_unavailability_and_cancellation() {
    use super::super::event_context::diagnostics::setup;
    use layer2_core::bots::BotError;
    let capture = Capture::default();
    let _guard = tracing::subscriber::set_default(capture.clone());
    ExternalContextObservation::from_hash("a".repeat(64).into())
        .unwrap()
        .scope(async {
            for (result, outcome, reason) in [
                (Ok(Some(())), "completed", "retained"),
                (Ok(None), "completed", "absent"),
                (Err(BotError::Forbidden), "error", "authority_refusal"),
                (Err(BotError::Unavailable), "error", "unavailable"),
            ] {
                let expected_ok = result.is_ok();
                let result = setup(async {
                    assert_eq!(DiagnosticContext::current().phase, "setup_context");
                    tokio::time::sleep(Duration::from_millis(3200)).await;
                    result
                })
                .await;
                assert_eq!(result.is_ok(), expected_ok);
                let record = capture.finished("setup_context");
                assert_eq!(record["outcome"], outcome);
                assert_eq!(record["reason"], reason);
                assert_eq!(record["elapsed_ms"], "3200");
            }
            {
                let future = setup(std::future::pending::<Result<Option<()>, BotError>>());
                tokio::pin!(future);
                assert!(futures_util::poll!(&mut future).is_pending());
                tokio::time::advance(Duration::from_secs(3)).await;
            }
            let record = capture.finished("setup_context");
            assert_eq!(record["outcome"], "cancelled");
            assert_eq!(record["elapsed_ms"], "3000");
        })
        .await;
}

#[tokio::test]
async fn handler_and_setup_diagnostics_are_absent_without_the_capability() {
    use super::super::event_context::diagnostics::{setup, Phase};
    let capture = Capture::default();
    let _guard = tracing::subscriber::set_default(capture.clone());
    let mut handler = Phase::new("handler");
    assert_eq!(
        setup(async {
            assert_eq!(DiagnosticContext::current().phase, "unspecified");
            Ok(Some(42))
        })
        .await
        .unwrap(),
        Some(42)
    );
    handler.response(StatusCode::OK);
    drop(handler);
    assert!(capture.0.lock().unwrap().is_empty());
}

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
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Logs {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

#[tokio::test]
async fn handler_and_setup_json_logs_exclude_current_request_ancestry() {
    use super::super::event_context::diagnostics::{setup, Phase};
    let logs = Logs::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(logs.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let parent = tracing::info_span!(
        "http.request",
        event_id = "private-event",
        uri = "/private/path",
        credential = "private-credential"
    );
    let _entered = parent.enter();
    let hash = "a".repeat(64);
    ExternalContextObservation::from_hash(hash.clone().into())
        .unwrap()
        .scope(async {
            let mut handler = Phase::new("handler");
            setup(async { Ok::<_, layer2_core::bots::BotError>(None::<()>) })
                .await
                .unwrap();
            handler.response(StatusCode::NOT_FOUND);
        })
        .await;
    let output = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert_eq!(output.lines().count(), 4);
    for line in output.lines() {
        let record: Value = serde_json::from_str(line).unwrap();
        assert_eq!(record["fields"]["attempt_hash"], hash);
    }
    for private in ["private-event", "/private/path", "private-credential"] {
        assert!(
            !output.contains(private),
            "inherited request field: {private}"
        );
    }
}

pub(super) async fn scope(enabled: bool, work: impl std::future::Future<Output = ()>) {
    if enabled {
        ExternalContextObservation::from_hash("c".repeat(64).into())
            .unwrap()
            .scope(work)
            .await;
    } else {
        work.await;
    }
}
