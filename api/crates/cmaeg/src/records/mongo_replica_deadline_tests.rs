//! Exercise the real direct admission-root route, replacing only Mongo attempts.
use super::super::replica_observation::{Observation, Phase};
use super::{AttemptError, WRITE_TIMEOUT};
use crate::records::{
    mongo::{MongoRecords, ReplicaKind, ReplicaWrite, INTERACTIONS},
    AdmissionRootWrite, RecordClient, RecordError, RecordKind,
};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::time::Instant;

#[derive(Clone, Copy, Debug)]
enum Outcome {
    Success,
    Aborted,
    Ambiguous,
    BothLabels,
    CommitBeforeReply,
    Conflict,
}

#[derive(Debug)]
pub(in crate::records::mongo) struct AttemptFixture {
    steps: Mutex<VecDeque<(u64, Outcome)>>,
    started: AtomicUsize,
    committed: AtomicBool,
}

impl AttemptFixture {
    pub(in crate::records::mongo) async fn attempt(
        &self,
        writes: &[ReplicaWrite],
        observation: &Observation,
    ) -> Result<Vec<u64>, AttemptError> {
        // The production root adapter must keep the same identity, payload and
        // expected version on every attempt; there is no outer Store::batch.
        assert_eq!(writes.len(), 1);
        let write = &writes[0];
        assert_eq!(write.id, "10000000-0000-4000-8000-000000000001");
        assert_eq!(write.owner, "fixture-owner");
        assert_eq!(write.blob, b"sealed-fixture");
        assert_eq!(write.expected, None);
        assert_eq!(write.expires_at, Some(1_800_000_000));
        assert!(write.guard.is_none() && write.channel.is_none() && write.condition.is_none());
        let projection = write.projection.as_ref().unwrap();
        assert!(matches!(projection.kind, ReplicaKind::Admission));
        assert!(projection.active);
        assert_eq!(projection.deployment, "a".repeat(64));
        let (delay_ms, outcome) = self.steps.lock().unwrap().pop_front().unwrap();
        self.started.fetch_add(1, Ordering::SeqCst);
        if matches!(outcome, Outcome::CommitBeforeReply) {
            self.committed.store(true, Ordering::SeqCst);
        }
        observation.phase(Phase::Commit);
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        let mongo_error = |labels: &[&str]| {
            let error = super::tests::error(labels);
            observation.mongo_error(&error);
            AttemptError::from(error)
        };
        match outcome {
            Outcome::Success | Outcome::CommitBeforeReply => Ok(vec![1]),
            Outcome::Aborted => Err(mongo_error(&["TransientTransactionError"])),
            Outcome::Ambiguous => Err(mongo_error(&["UnknownTransactionCommitResult"])),
            Outcome::BothLabels => Err(mongo_error(&[
                "TransientTransactionError",
                "UnknownTransactionCommitResult",
            ])),
            Outcome::Conflict => Err(RecordError::Conflict {
                expected: 0,
                actual: Some(1),
            }
            .into()),
        }
    }
}

async fn fixture(steps: &[(u64, Outcome)]) -> (RecordClient, Arc<AttemptFixture>) {
    let attempt = Arc::new(AttemptFixture {
        steps: Mutex::new(steps.iter().copied().collect()),
        started: AtomicUsize::new(0),
        committed: AtomicBool::new(false),
    });
    // A closed driver supplies inert typed handles. No database or container is
    // needed; any accidental real operation fails instead of reaching a server.
    let options = mongodb::options::ClientOptions::builder()
        .hosts(vec![mongodb::options::ServerAddress::Tcp {
            host: "127.0.0.1".into(),
            port: Some(1),
        }])
        .build();
    let client = mongodb::Client::with_options(options).unwrap();
    let db = client.database("replica_deadline_fixture");
    client.shutdown().await;
    let database = MongoRecords {
        records: db.collection(INTERACTIONS),
        db,
        kind: RecordKind::Interaction,
        worker_credentials: None,
        replica_attempt_fixture: Some(attempt.clone()),
    };
    let records = RecordClient::new("http://127.0.0.1:1", "fixture-service")
        .unwrap()
        .with_database(database)
        .interactions();
    (records, attempt)
}

async fn admit(records: &RecordClient) -> Result<u64, RecordError> {
    records
        .put_admission_root(AdmissionRootWrite {
            id: "10000000-0000-4000-8000-000000000001",
            owner: "fixture-owner",
            blob: b"sealed-fixture",
            expires_at: 1_800_000_000,
            deployment: &"a".repeat(64),
            github_guard: None,
        })
        .await
}

fn assert_deadline(result: Result<u64, RecordError>, started: Instant) {
    assert_eq!(
        result,
        Err(RecordError::Unavailable(
            "replica transaction deadline exceeded".into()
        ))
    );
    assert_eq!(started.elapsed(), WRITE_TIMEOUT);
}

#[tokio::test(start_paused = true)]
async fn direct_root_slow_attempt_is_bounded_without_assuming_no_commit() {
    let (records, attempt) = fixture(&[(10_000, Outcome::CommitBeforeReply)]).await;
    let started = Instant::now();
    assert_deadline(admit(&records).await, started);
    assert_eq!(attempt.started.load(Ordering::SeqCst), 1);
    assert!(attempt.committed.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn direct_root_aborted_attempts_share_one_total_budget() {
    let (records, attempt) = fixture(&[(800, Outcome::Aborted); 5]).await;
    let started = Instant::now();
    assert_deadline(admit(&records).await, started);
    assert_eq!(attempt.started.load(Ordering::SeqCst), 3);
}

#[tokio::test(start_paused = true)]
async fn direct_root_backoff_cannot_start_an_attempt_after_deadline() {
    let (records, attempt) = fixture(&[(1_999, Outcome::Aborted)]).await;
    let started = Instant::now();
    assert_deadline(admit(&records).await, started);
    assert_eq!(attempt.started.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn direct_root_ambiguous_commit_never_retries_even_with_aborted_label() {
    for outcome in [Outcome::Ambiguous, Outcome::BothLabels] {
        let (records, attempt) = fixture(&[(10, outcome)]).await;
        let started = Instant::now();
        assert!(matches!(
            admit(&records).await,
            Err(RecordError::Unavailable(_))
        ));
        assert!(started.elapsed() < WRITE_TIMEOUT);
        assert_eq!(attempt.started.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn direct_root_known_abort_can_succeed_within_budget() {
    let (records, attempt) = fixture(&[(10, Outcome::Aborted), (10, Outcome::Success)]).await;
    let started = Instant::now();
    assert_eq!(admit(&records).await, Ok(1));
    assert!(started.elapsed() < WRITE_TIMEOUT);
    assert_eq!(attempt.started.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn direct_root_fast_aborts_still_stop_at_five_attempts() {
    let (records, attempt) = fixture(&[(10, Outcome::Aborted); 5]).await;
    let started = Instant::now();
    assert_eq!(
        admit(&records).await,
        Err(RecordError::Unavailable(
            "replica transaction contention budget exhausted".into()
        ))
    );
    assert!(started.elapsed() < WRITE_TIMEOUT);
    assert_eq!(attempt.started.load(Ordering::SeqCst), 5);
}

#[tokio::test(start_paused = true)]
async fn direct_root_version_conflict_returns_to_caller_reconciliation() {
    let (records, attempt) = fixture(&[(10, Outcome::Conflict)]).await;
    assert_eq!(
        admit(&records).await,
        Err(RecordError::Conflict {
            expected: 0,
            actual: Some(1)
        })
    );
    assert_eq!(attempt.started.load(Ordering::SeqCst), 1);
}

#[path = "mongo_replica_observation_tests.rs"]
mod observation_tests;
