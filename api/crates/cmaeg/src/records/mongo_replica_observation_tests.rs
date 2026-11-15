//! Check actual direct-root diagnostics, including task cancellation and log isolation.
use super::{admit, fixture, Outcome};
use crate::records::{
    mongo::replica_observation::{Observation, Phase},
    RecordClient, RecordError,
};
use std::sync::{atomic::Ordering, Arc, Mutex};
use tracing::{instrument::WithSubscriber, Instrument};

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
            .with_max_level(tracing::Level::DEBUG)
            .without_time()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish()
    }
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

fn span(attempt: char) -> tracing::Span {
    tracing::info_span!(
        "eg_admission_attempt",
        attempt_sha256 = %attempt.to_string().repeat(64),
        process_sha256 = %"d".repeat(64),
        admission_sha256 = %"c".repeat(64)
    )
}

async fn traced_admit(
    records: &RecordClient,
    attempt: char,
    operation: char,
) -> Result<u64, RecordError> {
    async {
        async {
            admit(records)
                .instrument(tracing::info_span!(
                    "eg_admission_operation",
                    operation_sha256 = %operation.to_string().repeat(64),
                    stage = "root_write"
                ))
                .await
        }
        .instrument(span(attempt))
        .await
    }
    .instrument(tracing::info_span!(
        "cmaeg.request",
        request_id = "20000000-0000-4000-8000-000000000002"
    ))
    .await
}

fn assert_join(line: &str, attempt: char, operation: char) {
    for expected in [
        "eg_admission_attempt".to_owned(),
        "eg_admission_operation".to_owned(),
        format!("attempt_sha256={}", attempt.to_string().repeat(64)),
        format!("operation_sha256={}", operation.to_string().repeat(64)),
        format!("process_sha256={}", "d".repeat(64)),
        format!("admission_sha256={}", "c".repeat(64)),
        "stage=\"root_write\"".to_owned(),
        "request_id=\"20000000-0000-4000-8000-000000000002\"".to_owned(),
    ] {
        assert!(
            line.contains(&expected),
            "missing expected enclosing join: {expected}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn unknown_commit_labels_and_request_join_survive_without_a_retry() {
    for outcome in [Outcome::Ambiguous, Outcome::BothLabels] {
        let logs = Logs::default();
        let (records, attempt) = fixture(&[(10, outcome)]).await;
        traced_admit(&records, 'a', 'e')
            .with_subscriber(logs.subscriber())
            .await
            .unwrap_err();
        assert_eq!(attempt.started.load(Ordering::SeqCst), 1);
        let log = logs.text();
        let error = log
            .lines()
            .find(|line| line.contains("label_class=\"unknown_commit\""))
            .unwrap();
        assert_join(error, 'a', 'e');
        assert!(error.contains("io_class=\"commit\""));
        assert!(error.contains("phase=\"commit\"") && error.contains("transaction_attempt=1"));
        assert!(
            error.contains("commit_class=\"unknown\"")
                && error.contains("unknown_commit_label=true")
        );
        assert!(error.contains(if matches!(outcome, Outcome::BothLabels) {
            "transient_transaction_label=true"
        } else {
            "transient_transaction_label=false"
        }));
        for private in [
            "fixture-owner",
            "sealed-fixture",
            "10000000-0000",
            "fixture-service",
        ] {
            assert!(
                !log.contains(private),
                "private fixture content reached logs"
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn known_abort_and_success_have_distinct_attempts_and_operation_identity() {
    let logs = Logs::default();
    let (records, attempt) = fixture(&[(10, Outcome::Aborted), (10, Outcome::Success)]).await;
    assert_eq!(
        traced_admit(&records, 'a', 'e')
            .with_subscriber(logs.subscriber())
            .await,
        Ok(1)
    );
    assert_eq!(attempt.started.load(Ordering::SeqCst), 2);
    let log = logs.text();
    assert!(log.contains("label_class=\"transient_transaction\""));
    assert!(log.contains("commit_class=\"aborted\""));
    assert!(log.contains("phase=\"backoff\""));
    let end = log
        .lines()
        .find(|line| line.contains("outcome=\"ok\""))
        .unwrap();
    assert!(end.contains("transaction_attempt=2") && end.contains("commit_class=\"acknowledged\""));
    for line in log.lines() {
        assert_join(line, 'a', 'e');
    }
    let sequences: std::collections::HashSet<_> = log
        .lines()
        .map(|line| {
            line.split("record_operation_sequence=")
                .nth(1)
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
        })
        .collect();
    assert_eq!(sequences.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn deadline_and_outer_cancellation_never_report_an_aborted_commit() {
    for outer_cancel in [false, true] {
        let logs = Logs::default();
        let (records, attempt) = fixture(&[(10_000, Outcome::CommitBeforeReply)]).await;
        async {
            let call = traced_admit(&records, 'a', 'e');
            if outer_cancel {
                assert!(
                    tokio::time::timeout(std::time::Duration::from_millis(50), call)
                        .await
                        .is_err()
                );
            } else {
                assert!(call.await.is_err());
            }
        }
        .with_subscriber(logs.subscriber())
        .await;
        assert!(attempt.committed.load(Ordering::SeqCst));
        assert_eq!(attempt.started.load(Ordering::SeqCst), 1);
        let log = logs.text();
        let marker = if outer_cancel {
            "boundary=\"cancelled\""
        } else {
            "boundary=\"deadline\""
        };
        let line = log.lines().find(|line| line.contains(marker)).unwrap();
        assert!(line.contains("phase=\"commit\"") && line.contains("commit_class=\"unknown\""));
        assert_join(line, 'a', 'e');
        assert!(!log.contains("commit_class=\"aborted\"") && !log.contains("outcome=\"ok\""));
    }
}

#[tokio::test(start_paused = true)]
async fn concurrent_attempts_for_one_admission_keep_distinct_span_joins() {
    let logs = Logs::default();
    let (left, _) = fixture(&[(10, Outcome::Ambiguous)]).await;
    let (right, _) = fixture(&[(20, Outcome::Success)]).await;
    async {
        let (left, right) = tokio::join!(
            traced_admit(&left, 'a', 'e'),
            traced_admit(&right, 'b', 'f')
        );
        assert!(left.is_err());
        assert_eq!(right, Ok(1));
    }
    .with_subscriber(logs.subscriber())
    .await;
    let log = logs.text();
    for line in log.lines().filter(|line| line.contains("boundary=\"end\"")) {
        assert!(line.contains(&"c".repeat(64)));
        if line.contains("outcome=\"ok\"") {
            assert_join(line, 'b', 'f');
            assert!(line.contains(&"b".repeat(64)) && !line.contains(&"a".repeat(64)));
            assert!(!line.contains(&"e".repeat(64)));
        } else {
            assert_join(line, 'a', 'e');
            assert!(line.contains(&"a".repeat(64)) && !line.contains(&"b".repeat(64)));
            assert!(!line.contains(&"f".repeat(64)));
        }
    }
    assert_eq!(
        log.lines()
            .filter(|line| line.contains("boundary=\"end\""))
            .count(),
        2
    );
}

#[tokio::test]
async fn index_failure_retains_its_own_operation_and_update_phase() {
    let logs = Logs::default();
    let (records, _) = fixture(&[]).await;
    async {
        records
            .finish_admission_index(
                "10000000-0000-4000-8000-000000000001",
                "fixture-owner",
                1,
                &"a".repeat(64),
            )
            .instrument(span('a'))
            .await
    }
    .with_subscriber(logs.subscriber())
    .await
    .unwrap_err();
    let log = logs.text();
    assert!(log.contains("record_operation=\"admission_index\""));
    assert!(log.contains("phase=\"index_update\""));
    assert!(log.contains("io_class=\"write\""));
    assert!(log.contains("label_class=\"unlabelled_mongo\""));
    assert!(!log.contains("fixture-owner") && !log.contains("10000000-0000"));
}

#[test]
fn index_labels_do_not_classify_a_nontransactional_write_as_aborted() {
    let logs = Logs::default();
    tracing::subscriber::with_default(logs.subscriber(), || {
        let observation = Observation::index();
        observation.phase(Phase::IndexUpdate);
        observation.mongo_error(&super::super::tests::error(&["TransientTransactionError"]));
        observation.finish::<()>(&Err(RecordError::Unavailable("private-error".into())));
    });
    let log = logs.text();
    let error = log
        .lines()
        .find(|line| line.contains("boundary=\"error\""))
        .unwrap();
    assert!(error.contains("transient_transaction_label=true"));
    assert!(error.contains("label_class=\"transient_transaction\""));
    assert!(error.contains("commit_class=\"unknown\""));
    assert!(!log.contains("commit_class=\"aborted\"") && !log.contains("private-error"));
}

#[test]
fn error_fields_are_fixed_and_do_not_require_an_http_or_admission_parent() {
    let logs = Logs::default();
    tracing::subscriber::with_default(logs.subscriber(), || {
        let observation = Observation::transaction();
        observation.attempt(1);
        observation.phase(Phase::RecordInsert);
        let concern: mongodb::error::WriteConcernError =
            serde_json::from_value(serde_json::json!({
                "code": 91, "codeName": "private-code", "errmsg": "private-error",
                "errInfo": {"private-owner": "private-value"},
                "errorLabels": ["private-label"],
            }))
            .unwrap();
        let error = mongodb::error::ErrorKind::Write(
            mongodb::error::WriteFailure::WriteConcernError(concern),
        )
        .into();
        observation.mongo_error(&error);
        observation.finish::<()>(&Err(RecordError::GuardConflict {
            scope: "private-scope",
            expected: 123456789,
        }));
    });
    let log = logs.text();
    assert!(log.contains("label_class=\"unlabelled_mongo\""));
    assert!(log.contains("error_class=\"guard_conflict\""));
    for forbidden in [
        "private",
        "123456789",
        "attempt_sha256",
        "process_sha256",
        "admission_sha256",
        "request_id",
    ] {
        assert!(
            !log.contains(forbidden),
            "unexpected identifying/error field"
        );
    }
}
