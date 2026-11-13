use super::*;
use crate::ingress::{
    routing_plan::fixture,
    routing_store::{test_server::Server, Candidate},
};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::Duration,
};
use tracing::instrument::WithSubscriber;

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
impl Logs {
    fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync + 'static {
        let writer = self.clone();
        tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(move || writer.clone())
            .finish()
    }
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}
fn fields(text: &str, name: &str) -> BTreeSet<String> {
    let prefix = format!("{name}=\"");
    text.lines()
        .filter_map(|line| line.split_once(&prefix))
        .map(|(_, rest)| rest.split('"').next().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn concurrent_replays_have_separate_attempts_and_one_event_process_identity() {
    let request = fixture::request(fixture::targets, fixture::message("private-payload-canary"));
    let logs = Logs::default();
    async {
        let work = || async {
            admission(Stage::Enqueue, async {
                tokio::task::yield_now().await;
                Ok::<_, AdmissionError>(17)
            })
            .await
            .unwrap();
            Ok::<_, Problem>(23)
        };
        let (first, second) = tokio::join!(attempt(&request, work()), attempt(&request, work()));
        assert_eq!(first.unwrap(), 23);
        assert_eq!(second.unwrap(), 23);
    }
    .with_subscriber(logs.subscriber())
    .await;
    let text = logs.text();
    let attempts = fields(&text, "attempt_sha256");
    assert_eq!(attempts.len(), 2, "{text}");
    assert_eq!(fields(&text, "process_sha256").len(), 1);
    assert_eq!(fields(&text, "admission_sha256").len(), 1);
    assert_eq!(fields(&text, "operation_sha256").len(), 2);
    for field in attempts {
        assert_eq!(field.len(), 64);
        assert!(field.bytes().all(|b| b.is_ascii_hexdigit()));
        let lines: Vec<_> = text.lines().filter(|line| line.contains(&field)).collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.contains("boundary=\"end\""))
                .count(),
            2
        );
    }
    assert!(!text.contains("private-payload-canary"));
    assert!(!text.contains(request.channel.definition.owner.subject.as_str()));
}

#[tokio::test]
async fn errors_are_unchanged_and_raw_details_never_enter_observations() {
    let request = fixture::request(fixture::targets, fixture::message("private-payload-canary"));
    let logs = Logs::default();
    async {
        for error in [
            RecordError::NotFound,
            RecordError::Conflict {
                expected: 7,
                actual: Some(9),
            },
            RecordError::GuardConflict {
                scope: "private-scope-canary",
                expected: 8,
            },
            RecordError::Unavailable("private-error-canary".into()),
            RecordError::Rejected("private-error-canary".into()),
            RecordError::Malformed("private-error-canary".into()),
        ] {
            attempt(&request, async {
                assert_eq!(
                    record(Stage::RootWrite, async { Err::<(), _>(error.clone()) }).await,
                    Err(error)
                );
                Ok::<_, Problem>(())
            })
            .await
            .unwrap();
        }
        for error in [
            AdmissionError::Invalid,
            AdmissionError::TooLarge,
            AdmissionError::Conflict,
            AdmissionError::GuardConflict,
            AdmissionError::Expired,
            AdmissionError::ExpiredOrUnavailable,
            AdmissionError::Unavailable,
            AdmissionError::Uncertain,
            AdmissionError::Corrupt,
            AdmissionError::MissingChunk,
        ] {
            attempt(&request, async {
                assert_eq!(
                    admission(Stage::Enqueue, async { Err::<(), _>(error.clone()) }).await,
                    Err(error)
                );
                Ok::<_, Problem>(())
            })
            .await
            .unwrap();
        }
    }
    .with_subscriber(logs.subscriber())
    .await;
    let text = logs.text();
    assert!(!text.contains("private-"));
    for class in [
        "not_found",
        "conflict",
        "guard_conflict",
        "unavailable",
        "rejected",
        "malformed",
        "invalid",
        "too_large",
        "expired",
        "expired_or_unavailable",
        "uncertain",
        "corrupt",
        "missing_chunk",
    ] {
        assert!(text.contains(&format!("class=\"{class}\"")), "{class}");
    }
}

#[tokio::test]
async fn cancellation_is_uncertain_and_does_not_emit_a_completed_stage() {
    let request = fixture::request(fixture::targets, fixture::message("message"));
    let logs = Logs::default();
    async {
        let result = tokio::time::timeout(
            Duration::from_millis(10),
            attempt(&request, async {
                record(
                    Stage::RootWrite,
                    std::future::pending::<Result<(), RecordError>>(),
                )
                .await
                .unwrap();
                Ok::<_, Problem>(())
            }),
        )
        .await;
        assert!(result.is_err());
    }
    .with_subscriber(logs.subscriber())
    .await;
    let text = logs.text();
    assert_eq!(text.lines().count(), 4, "{text}");
    assert_eq!(
        text.lines()
            .filter(|line| line.contains("boundary=\"cancelled\" class=\"uncertain\""))
            .count(),
        2
    );
    assert!(!text.contains("boundary=\"end\""));
}

#[tokio::test]
async fn background_reads_do_not_invent_an_admission_attempt() {
    let logs = Logs::default();
    let result = admission(Stage::RootLookup, async { Ok::<_, AdmissionError>(37) })
        .with_subscriber(logs.subscriber())
        .await;
    assert_eq!(result, Ok(37));
    assert!(logs.text().is_empty());
}

#[tokio::test]
async fn index_failure_preserves_the_refusal_without_rendering_its_detail() {
    let request = fixture::request(fixture::targets, fixture::message("message"));
    let logs = Logs::default();
    let result = attempt(&request, async {
        record(Stage::IndexRetire, async {
            Err::<(), _>(RecordError::Unavailable("private-error-canary".into()))
        })
        .await
        .map_err(|_| {
            Problem::new(
                cmaeg_core::problem::ProblemCode::ProviderUnavailable,
                503,
                "private-detail-canary",
            )
        })
    })
    .with_subscriber(logs.subscriber())
    .await;
    let problem = result.unwrap_err();
    assert_eq!(problem.status, 503);
    let text = logs.text();
    assert!(text.contains("stage=\"index_retire\""));
    assert!(text.contains("class=\"unavailable\""));
    assert!(text.contains("class=\"refused\""));
    assert!(text.contains("public_status=503"));
    assert!(!text.contains("private-"));
}

#[tokio::test]
async fn real_store_root_write_readback_and_restored_replay_keep_the_attempt_join() {
    let server = Server::start().await;
    let request = fixture::request(fixture::targets, fixture::message("private-payload-canary"));
    let candidate = Candidate::prepare(
        &request,
        &fixture::routed(&request),
        b"private-source-canary",
        &fixture::keys(),
        fixture::NOW,
    )
    .unwrap();
    server.fault(candidate.admission_id().into(), true, false);
    let logs = Logs::default();
    async {
        let first = attempt(&request, async {
            Ok::<_, Problem>(server.store().admit(&candidate).await.unwrap())
        })
        .await
        .unwrap();
        let replay = attempt(&request, async {
            Ok::<_, Problem>(
                server
                    .store()
                    .replay(&request, b"private-source-canary")
                    .await
                    .unwrap()
                    .unwrap(),
            )
        })
        .await
        .unwrap();
        assert_eq!(first, replay);
    }
    .with_subscriber(logs.subscriber())
    .await;
    let text = logs.text();
    assert_eq!(fields(&text, "attempt_sha256").len(), 2);
    assert_eq!(fields(&text, "admission_sha256").len(), 1);
    for stage in [
        "root_lookup",
        "chunk_write",
        "chunk_read",
        "root_write",
        "root_readback",
    ] {
        assert!(
            text.contains(&format!("stage=\"{stage}\"")),
            "{stage}: {text}"
        );
    }
    assert!(text.contains("class=\"unavailable\""));
    assert!(!text.contains("private-"));
}
