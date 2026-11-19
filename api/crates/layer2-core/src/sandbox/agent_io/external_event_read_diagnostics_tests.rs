use super::*;
use std::{io::Write, sync::Mutex, time::Duration};

#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);
impl Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend(bytes);
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
impl Logs {
    fn records(&self) -> Vec<serde_json::Value> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

#[tokio::test(start_paused = true)]
async fn default_observation_preserves_completed_phase_and_total_drop_baseline() {
    let logs = Logs::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(logs.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    observe("retained_command_lookup", async {
        assert_eq!(DiagnosticContext::current().phase, "unspecified");
        Ok(())
    })
    .await
    .unwrap();
    let result = observe::<()>("retained_command_recheck", async {
        Err(CommandLedgerError::Unavailable.into())
    })
    .await;
    assert!(result.is_err());
    {
        let future = observe(
            "total",
            observe(
                "resource_authorization",
                std::future::pending::<Result<(), CommandAdmissionError>>(),
            ),
        );
        tokio::pin!(future);
        assert!(futures_util::poll!(&mut future).is_pending());
        tokio::time::advance(Duration::from_secs(3)).await;
    }
    let records = logs.records();
    let phases: Vec<_> = records
        .iter()
        .map(|record| record["fields"]["phase"].as_str().unwrap())
        .collect();
    assert_eq!(
        phases,
        [
            "retained_command_lookup",
            "retained_command_recheck",
            "total"
        ]
    );
    for record in records {
        let fields = record["fields"].as_object().unwrap();
        assert!(fields.contains_key("elapsed_ms"));
        assert!(!fields.contains_key("outcome"));
        assert!(!fields.contains_key("attempt_hash"));
    }
}

#[tokio::test(start_paused = true)]
async fn phase_timing_does_not_inherit_request_identity() {
    let logs = Logs::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(logs.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let request = tracing::info_span!(
        "http.request",
        request_id = "request-identity",
        uri =
            "/bindings/binding-identity/conversations/conversation-identity/events/event-identity"
    );
    let _entered = request.enter();
    ExternalContextObservation::from_hash("b".repeat(64).into())
        .unwrap()
        .scope(observe(
            "total",
            observe("retained_command_lookup", async { Ok(()) }),
        ))
        .await
        .unwrap();
    let output = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert_eq!(output.lines().count(), 4);
    for phase in ["retained_command_lookup", "total"] {
        assert!(output.contains(phase));
    }
    for private in [
        "request-identity",
        "binding-identity",
        "conversation-identity",
        "event-identity",
    ] {
        assert!(
            !output.contains(private),
            "inherited request field: {private}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn every_phase_records_completion_error_and_cancellation_with_its_duration() {
    let logs = Logs::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(logs.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let hash = "a".repeat(64);
    let context = ExternalContextObservation::from_hash(hash.clone().into()).unwrap();
    context
        .scope(async {
            for phase in [
                "retained_command_lookup",
                "resource_authorization",
                "retained_command_recheck",
                "final_release_fence",
                "total",
            ] {
                for outcome in ["completed", "error", "cancelled"] {
                    let work = async {
                        assert_eq!(DiagnosticContext::current().phase, phase);
                        tokio::time::sleep(Duration::from_millis(3200)).await;
                        if outcome == "error" {
                            Err(CommandLedgerError::Unavailable.into())
                        } else {
                            Ok(())
                        }
                    };
                    if outcome == "cancelled" {
                        let future = observe(phase, work);
                        tokio::pin!(future);
                        assert!(futures_util::poll!(&mut future).is_pending());
                        tokio::time::advance(Duration::from_millis(3000)).await;
                    } else {
                        let result = observe(phase, work).await;
                        assert_eq!(result.is_ok(), outcome == "completed");
                    }
                    let records = logs.records();
                    let fields = &records.last().unwrap()["fields"];
                    assert_eq!(fields["phase"], phase);
                    assert_eq!(fields["outcome"], outcome);
                    assert_eq!(fields["attempt_hash"], hash);
                    assert_eq!(
                        fields["elapsed_ms"],
                        if outcome == "cancelled" { 3000 } else { 3200 }
                    );
                    assert_eq!(DiagnosticContext::current().phase, "unspecified");
                }
            }
        })
        .await;
    assert_eq!(DiagnosticContext::current().request_hash.as_ref(), "none");
    assert!(ExternalContextObservation::current().is_none());
}

#[tokio::test]
async fn unvalidated_observation_context_cannot_emit_identifiers() {
    let logs = Logs::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(logs.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    for invalid in [
        "private-input",
        &"A".repeat(64),
        &"a".repeat(63),
        &"0".repeat(65),
    ] {
        assert!(ExternalContextObservation::from_hash(invalid.into()).is_none());
    }
    let hash = "b".repeat(64);
    ExternalContextObservation::from_hash(hash.clone().into())
        .unwrap()
        .scope(
            DiagnosticContext {
                request_hash: "private-input".into(),
                ..Default::default()
            }
            .scope(observe("total", async { Ok(()) })),
        )
        .await
        .unwrap();
    assert!(logs
        .records()
        .iter()
        .all(|record| record["fields"]["attempt_hash"] == hash));
}
