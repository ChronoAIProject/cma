//! Adversarial persistence recovery and store-contract tests.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

use crate::event::{Event, EventKind, StoredEvent};
use crate::ids::{SessionId, SubjectId};
use crate::options::SessionMode;
use crate::session::{RecoveryState, Session, SessionStatus, StopReason};
use crate::store::{SessionRecord, SessionStore, StoreError};
use crate::test_support::{await_settled, service_over, spec, user, FakeAdapter, FakeStore};
use crate::{AgentOptions, KernelId, Reserved, TerminalGeometry};

fn id() -> SessionId {
    SessionId("sesn_log_recovery".into())
}

fn record() -> SessionRecord {
    SessionRecord::new(Session {
        id: id(),
        owner: SubjectId("subject-log".into()),
        kernel: KernelId::new("fake"),
        status: SessionStatus::Ready,
        recovery_state: RecoveryState::Normal,
        stop_reason: None,
        options: AgentOptions::default(),
        environment_profile: None,
        geometry: TerminalGeometry::default(),
        last_seq: 0,
        runtime_version: None,
        rotations: 0,
        rotated_at: None,
        capabilities: Default::default(),
        granted_services: None,
        granted_skills: None,
        pricing: None,
        created_at: "2026-08-24T00:00:00Z".into(),
    })
}

fn said(seq: u64, content: &str) -> Event {
    said_for(&id(), seq, content)
}

fn said_for(id: &SessionId, seq: u64, content: &str) -> Event {
    Event {
        session_id: id.clone(),
        seq,
        kind: EventKind::MessageCommitted {
            content: content.into(),
        },
        appended_at: String::new(),
    }
}

#[derive(Default)]
struct FaultStore {
    inner: Arc<FakeStore>,
    failures: AtomicUsize,
    failures_after_first_write: AtomicUsize,
    calls: AtomicUsize,
    lose_next_response: AtomicBool,
    pause_next_write: AtomicBool,
    write_started: Notify,
    release_write: Notify,
}

impl FaultStore {
    fn consume_failure(counter: &AtomicUsize) -> bool {
        counter
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
    }

    async fn before_write(&self) -> Result<(), StoreError> {
        let earlier_calls = self.calls.fetch_add(1, Ordering::SeqCst);
        if self.pause_next_write.swap(false, Ordering::SeqCst) {
            self.write_started.notify_one();
            self.release_write.notified().await;
        }
        if Self::consume_failure(&self.failures)
            || (earlier_calls > 0 && Self::consume_failure(&self.failures_after_first_write))
        {
            return Err(StoreError::Unavailable("transient test failure".into()));
        }
        Ok(())
    }

    async fn maybe_lose<T>(&self, value: T) -> Result<T, StoreError> {
        if self.lose_next_response.swap(false, Ordering::SeqCst) {
            return Err(StoreError::Unavailable(
                "test response lost after commit".into(),
            ));
        }
        Ok(value)
    }
}

#[async_trait::async_trait]
impl SessionStore for FaultStore {
    async fn ping(&self) -> Result<(), StoreError> {
        self.inner.ping().await
    }

    async fn upsert(&self, record: &SessionRecord) -> Result<(), StoreError> {
        self.inner.upsert(record).await
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
        self.inner.get(id).await
    }

    async fn list_for(&self, owner: &SubjectId) -> Result<Vec<SessionRecord>, StoreError> {
        self.inner.list_for(owner).await
    }

    async fn reserved_capacity(&self) -> Result<Reserved, StoreError> {
        self.inner.reserved_capacity().await
    }

    async fn list_all(&self) -> Result<Vec<SessionRecord>, StoreError> {
        self.inner.list_all().await
    }

    async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
        self.inner.delete(id).await
    }

    async fn append_events(&self, events: &[Event]) -> Result<(), StoreError> {
        self.inner.append_events(events).await
    }

    async fn commit_session_idempotent(
        &self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError> {
        self.before_write().await?;
        let revision = self
            .inner
            .commit_session_idempotent(write_id, record, events)
            .await?;
        self.maybe_lose(revision).await
    }

    async fn create_session_idempotent(
        &self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError> {
        self.before_write().await?;
        let revision = self
            .inner
            .create_session_idempotent(write_id, record, events)
            .await?;
        self.maybe_lose(revision).await
    }

    async fn events_since(
        &self,
        id: &SessionId,
        after: u64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        self.inner.events_since(id, after).await
    }
}

async fn wait_for_event(store: &FakeStore) -> StoredEvent {
    wait_for_event_for(store, &id()).await
}

async fn wait_for_event_for(store: &FakeStore, id: &SessionId) -> StoredEvent {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(event) = store
                .events_since(id, 0)
                .await
                .expect("store readable")
                .into_iter()
                .next()
            {
                return event;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the autonomous worker committed its retained event")
}

async fn wait_for_lane_idle(lane: &Arc<super::SessionLog>) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while lane.queued() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the durable lane drains");
}

async fn collide_with_newer_record(
    base: SessionRecord,
    mut stale: SessionRecord,
    event: Event,
    newer: SessionRecord,
) -> (StoreError, SessionRecord, Vec<StoredEvent>) {
    let store = Arc::new(FaultStore::default());
    store.inner.upsert(&base).await.expect("seed record");
    store.pause_next_write.store(true, Ordering::SeqCst);
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));

    let task_service = Arc::clone(&service);
    let stale_write =
        tokio::spawn(async move { task_service.persist_durably(&mut stale, vec![event]).await });
    store.write_started.notified().await;
    store
        .inner
        .commit_session_idempotent("newer-competing-record", &newer, &[])
        .await
        .expect("the competing replica commits first");
    store.release_write.notify_one();

    let error = stale_write
        .await
        .expect("the stale writer task completes")
        .expect_err("the stale state change is refused");
    let durable = store
        .inner
        .get(&id())
        .await
        .expect("store remains readable")
        .expect("session remains present");
    let events = store.inner.events_since(&id(), 0).await.unwrap();
    (error, durable, events)
}

#[test]
fn an_informational_event_does_not_claim_a_pending_status_change() {
    let base = record();
    let mut intended = base.clone();
    intended.session.status = SessionStatus::Running;
    let message_change = super::RecordChange {
        before: Box::new(base.clone()),
        fields: super::RecordFields::for_events(&[said(1, "owner-message")]),
    };
    let mut rival = base;
    rival.context_ref = Some("rival-state".into());
    rival.revision = 1;

    let after_message = message_change
        .apply(&intended, rival)
        .expect("the informational event preserves the rival record");
    assert_eq!(after_message.session.status, SessionStatus::Ready);
    assert_eq!(after_message.context_ref.as_deref(), Some("rival-state"));
    let status_base = message_change.next_local_tail(&intended);
    assert_eq!(status_base.session.status, SessionStatus::Ready);

    let status_change = super::RecordChange {
        before: Box::new(status_base),
        fields: super::RecordFields::for_events(&[Event {
            session_id: id(),
            seq: 2,
            kind: EventKind::StatusChanged {
                status: SessionStatus::Running,
                stop_reason: None,
                detail: None,
                capability: None,
            },
            appended_at: String::new(),
        }]),
    };
    let after_status = status_change
        .apply(&intended, after_message)
        .expect("the semantic event owns the status change");
    assert_eq!(after_status.session.status, SessionStatus::Running);
    assert_eq!(after_status.context_ref.as_deref(), Some("rival-state"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sequence_allocation_cannot_be_inverted_before_lane_insertion() {
    let store = Arc::new(FakeStore::default());
    let base = record();
    store.upsert(&base).await.expect("seed record");
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let lane = service.event_logs.lane(&id());
    let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let (allocated_tx, allocated_rx) = tokio::sync::oneshot::channel();

    let first_lane = Arc::clone(&lane);
    let first_release = Arc::clone(&release);
    let first = tokio::task::spawn_blocking(move || {
        first_lane.enqueue_commit_with(
            super::CommitMode::UpdateExact,
            base,
            vec![said(0, "allocated-first")],
            move |last_seq| {
                let _ = allocated_tx.send(last_seq);
                let (released, wake) = &*first_release;
                let mut released = released.lock().unwrap();
                while !*released {
                    released = wake.wait(released).unwrap();
                }
            },
        )
    });
    assert_eq!(allocated_rx.await.expect("first allocation observed"), 1);

    let second_lane = Arc::clone(&lane);
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let second_record = record();
    let second = tokio::task::spawn_blocking(move || {
        let _ = started_tx.send(());
        second_lane.enqueue_commit(
            super::CommitMode::UpdateExact,
            second_record,
            vec![said(0, "enqueued-second")],
        )
    });
    started_rx.await.expect("second producer started");
    tokio::task::yield_now().await;
    assert_eq!(
        service.sequences.issued(&id(), 0),
        1,
        "the second producer cannot allocate while the first is paused before insertion"
    );

    let (released, wake) = &*release;
    *released.lock().unwrap() = true;
    wake.notify_all();
    let first = first.await.expect("first producer returns");
    let second = second.await.expect("second producer returns");
    first
        .complete
        .await
        .expect("first worker remains")
        .expect("first commit");
    second
        .complete
        .await
        .expect("second worker remains")
        .expect("second commit");

    let events = store.events_since(&id(), 0).await.unwrap();
    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        [1, 2]
    );
    assert!(format!("{:?}", events[0].kind).contains("allocated-first"));
    assert!(format!("{:?}", events[1].kind).contains("enqueued-second"));
}

#[tokio::test]
async fn cancelling_a_waiter_does_not_cancel_its_retained_write() {
    let store = Arc::new(FaultStore::default());
    let seeded = record();
    store.inner.upsert(&seeded).await.expect("seed record");
    store.failures.store(3, Ordering::SeqCst);
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));

    let task_service = Arc::clone(&service);
    let task = tokio::spawn(async move {
        let mut update = seeded;
        task_service
            .persist_durably(&mut update, vec![said(0, "survives-cancellation")])
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while store.calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("worker attempted the write");
    task.abort();
    let _ = task.await;

    let event = wait_for_event(&store.inner).await;
    assert!(format!("{:?}", event.kind).contains("survives-cancellation"));
    assert_eq!(store.inner.events_since(&id(), 0).await.unwrap().len(), 1);
}

#[tokio::test]
async fn an_error_delivered_to_the_waiter_ends_the_operation_permanently() {
    let store = Arc::new(FaultStore::default());
    let mut update = record();
    store.inner.upsert(&update).await.expect("seed record");
    store.failures.store(4, Ordering::SeqCst);
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));

    let error = service
        .persist_durably(&mut update, vec![said(0, "must-not-arrive-later")])
        .await
        .expect_err("the fourth unavailable result is reported");
    assert!(matches!(error, StoreError::Unavailable(_)));
    assert_eq!(store.calls.load(Ordering::SeqCst), 4);

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        store.calls.load(Ordering::SeqCst),
        4,
        "a reported operation must have no worker left that can commit it"
    );
    assert!(store.inner.events_since(&id(), 0).await.unwrap().is_empty());
    assert_eq!(store.inner.get(&id()).await.unwrap().unwrap().revision, 0);

    let mut successor = record();
    service
        .persist_durably(&mut successor, vec![said(0, "a-later-operation")])
        .await
        .expect("a later operation closes the uncommitted allocation gap");
    let events = store.inner.events_since(&id(), 0).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].seq, 1);
    assert!(format!("{:?}", events[0].kind).contains("a-later-operation"));
}

#[tokio::test]
async fn queued_successor_and_record_barrier_shift_with_an_abandoned_reservation() {
    let store = Arc::new(FaultStore::default());
    let base = record();
    store.inner.upsert(&base).await.expect("seed record");
    store.failures.store(4, Ordering::SeqCst);
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let lane = service.event_logs.lane(&id());
    lane.observe_record(&base);
    wait_for_lane_idle(&lane).await;

    let abandoned = lane.enqueue_commit(
        super::CommitMode::UpdateExact,
        base.clone(),
        vec![said(0, "abandoned")],
    );
    let successor = lane.enqueue_commit(
        super::CommitMode::UpdateExact,
        base.clone(),
        vec![said(0, "already-queued-successor")],
    );
    let mut live_snapshot = base;
    live_snapshot.session.last_seq = 2;
    let barrier = lane.enqueue_commit(
        super::CommitMode::UpdateInheriting,
        live_snapshot,
        Vec::new(),
    );

    assert!(matches!(
        abandoned
            .complete
            .await
            .expect("worker reports")
            .expect_err("the first operation is abandoned"),
        StoreError::Unavailable(_)
    ));
    successor
        .complete
        .await
        .expect("worker remains")
        .expect("the queued successor closes the reservation");
    let barrier_record = barrier
        .complete
        .await
        .expect("worker remains")
        .expect("the queued record-only barrier uses the corrected tail");

    assert_eq!(store.calls.load(Ordering::SeqCst), 6);
    assert_eq!(barrier_record.session.last_seq, 1);
    assert_eq!(service.sequences.current(&id()), Some(1));
    let events = store.inner.events_since(&id(), 0).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].seq, 1);
    assert!(format!("{:?}", events[0].kind).contains("already-queued-successor"));
}

#[tokio::test]
async fn cancelling_after_error_delivery_but_before_observation_retains_the_write() {
    let store = Arc::new(FaultStore::default());
    let seeded = record();
    store.inner.upsert(&seeded).await.expect("seed record");
    store.failures.store(4, Ordering::SeqCst);
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let lane = service.event_logs.lane(&id());
    let pending = lane.enqueue_commit(
        super::CommitMode::UpdateExact,
        seeded,
        vec![said(0, "unobserved-error-keeps-ownership")],
    );

    tokio::time::timeout(Duration::from_secs(1), lane.failure_delivered.notified())
        .await
        .expect("the fourth outage response is delivered");
    assert!(store.inner.events_since(&id(), 0).await.unwrap().is_empty());
    assert_eq!(store.calls.load(Ordering::SeqCst), 4);

    // The completion receiver still exists, so delivery succeeded, but it has
    // never been polled and therefore has not acknowledged the error.
    drop(pending.complete);

    let event = wait_for_event(&store.inner).await;
    assert!(format!("{:?}", event.kind).contains("unobserved-error-keeps-ownership"));
    assert_eq!(store.calls.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn a_failed_generated_creation_releases_its_allocator_key() {
    let store = Arc::new(FaultStore::default());
    store.failures.store(4, Ordering::SeqCst);
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));

    let error = service
        .create_session(&user("failed-create-owner"), spec())
        .await
        .expect_err("the fourth store outage response fails creation");

    assert!(matches!(error, crate::CoreError::RuntimeUnavailable(_)));
    assert_eq!(store.calls.load(Ordering::SeqCst), 4);
    assert_eq!(
        service.sequences.len(),
        0,
        "a generated identifier that never became a session must not remain strongly held"
    );
}

#[tokio::test]
async fn an_abandoned_gap_converges_the_allocator_live_entry_and_record_flush() {
    let store = Arc::new(FaultStore::default());
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let owner = user("gap-owner");
    let created = service
        .create_session(&owner, spec())
        .await
        .expect("session is created");
    await_settled(&service, &owner, &created.id).await;
    let lane = {
        let entries = service.entries.read().await;
        Arc::clone(
            &entries
                .get(&created.id)
                .expect("the settled session has a live entry")
                .event_log,
        )
    };
    wait_for_lane_idle(&lane).await;

    let before = store
        .inner
        .get(&created.id)
        .await
        .unwrap()
        .expect("settled record exists");
    let before_tail = before.session.last_seq;
    let calls_before_failure = store.calls.load(Ordering::SeqCst);
    store.failures.store(4, Ordering::SeqCst);
    let mut abandoned = before;
    let error = service
        .persist_durably(
            &mut abandoned,
            vec![said_for(&created.id, 0, "abandoned-gap-event")],
        )
        .await
        .expect_err("the fourth outage response abandons this operation");
    assert!(matches!(error, StoreError::Unavailable(_)));
    assert_eq!(store.calls.load(Ordering::SeqCst) - calls_before_failure, 4);

    {
        let mut entries = service.entries.write().await;
        let entry = entries
            .get_mut(&created.id)
            .expect("the session remains live");
        entry.context_ref = Some("record-only-after-gap".into());
        super::append(
            entry,
            EventKind::MessageCommitted {
                content: "successor-closes-gap".into(),
            },
        );
    }
    let calls_before_successor = store.calls.load(Ordering::SeqCst);
    service.flush(&created.id).await;
    assert_eq!(
        store.calls.load(Ordering::SeqCst) - calls_before_successor,
        2,
        "the successor and record-only flush each commit once without a rejected gap transaction"
    );

    let durable = store.inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.session.last_seq, before_tail + 1);
    assert_eq!(
        durable.context_ref.as_deref(),
        Some("record-only-after-gap")
    );
    let events = store
        .inner
        .events_since(&created.id, before_tail)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].seq, before_tail + 1);
    assert!(format!("{:?}", events[0].kind).contains("successor-closes-gap"));
    assert_eq!(
        service.sequences.current(&created.id),
        Some(before_tail + 1)
    );
    assert_eq!(
        service
            .entries
            .read()
            .await
            .get(&created.id)
            .expect("entry remains installed")
            .session
            .last_seq,
        before_tail + 1,
        "the live entry is corrected when the record-only barrier completes"
    );

    {
        let mut entries = service.entries.write().await;
        super::append(
            entries.get_mut(&created.id).unwrap(),
            EventKind::MessageCommitted {
                content: "allocator-stays-converged".into(),
            },
        );
    }
    let calls_before_followup = store.calls.load(Ordering::SeqCst);
    service.flush(&created.id).await;
    assert_eq!(
        store.calls.load(Ordering::SeqCst) - calls_before_followup,
        2,
        "later events remain on the contiguous fast path"
    );
    let events = store
        .inner
        .events_since(&created.id, before_tail)
        .await
        .unwrap();
    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        [before_tail + 1, before_tail + 2]
    );
}

#[tokio::test]
async fn a_revision_rebase_does_not_spend_the_unavailable_retry_budget() {
    let store = Arc::new(FaultStore::default());
    let mut durable = record();
    durable.revision = 1;
    store
        .inner
        .upsert(&durable)
        .await
        .expect("seed a newer durable revision");
    store.failures_after_first_write.store(4, Ordering::SeqCst);
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let mut stale = record();

    let error = tokio::time::timeout(
        Duration::from_secs(2),
        service.persist_durably(&mut stale, vec![said(0, "conflict-before-outage")]),
    )
    .await
    .expect("the active waiter is still reported to")
    .expect_err("four actual unavailable results are reported");

    assert!(matches!(error, StoreError::Unavailable(_)));
    assert_eq!(
        store.calls.load(Ordering::SeqCst),
        5,
        "one revision conflict precedes four unavailable writes"
    );
    assert!(store.inner.events_since(&id(), 0).await.unwrap().is_empty());
}

#[tokio::test]
async fn create_conflicts_are_not_rebased_as_updates() {
    let store = Arc::new(FakeStore::default());
    let mut duplicate = record();
    store
        .upsert(&duplicate)
        .await
        .expect("seed existing record");
    let service = service_over(store, Arc::new(FakeAdapter::default()));

    let result = tokio::time::timeout(
        Duration::from_secs(1),
        service.create_durably(&mut duplicate, vec![said(0, "duplicate-create")]),
    )
    .await
    .expect("a create collision does not spin");

    assert!(matches!(
        result,
        Err(StoreError::RevisionConflict {
            expected: 0,
            actual: 0
        })
    ));
}

#[tokio::test]
async fn a_lost_commit_response_is_reconciled_by_the_whole_write_receipt() {
    let store = Arc::new(FaultStore::default());
    let mut update = record();
    store.inner.upsert(&update).await.expect("seed record");
    store.lose_next_response.store(true, Ordering::SeqCst);
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));

    service
        .persist_durably(&mut update, vec![said(0, "uncertain-once")])
        .await
        .expect("the retry recognizes the committed transaction receipt");

    assert_eq!(update.revision, 1);
    assert_eq!(store.inner.events_since(&id(), 0).await.unwrap().len(), 1);
    assert_eq!(store.inner.get(&id()).await.unwrap().unwrap().revision, 1);
}

#[tokio::test]
async fn a_cross_replica_revision_change_rebases_the_semantic_write() {
    let store = Arc::new(FaultStore::default());
    let base = record();
    store.inner.upsert(&base).await.expect("seed record");
    store.failures.store(1, Ordering::SeqCst);
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));

    let task_service = Arc::clone(&service);
    let mut stale = base.clone();
    stale.context_ref = Some("stale-snapshot".into());
    let task = tokio::spawn(async move {
        task_service
            .persist_durably(&mut stale, vec![said(0, "stale-event")])
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while store.calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the older writer reached its retry worker");

    let mut newer = base;
    newer.context_ref = Some("newer-record-only-state".into());
    let revision = store
        .inner
        .commit_session_idempotent("external-record-only", &newer, &[])
        .await
        .expect("another writer advances the durable record");
    service.event_logs.lane(&id()).observe_revision(revision);

    task.await
        .expect("older task completes")
        .expect("its semantic event rebases onto the newer record");
    let durable = store.inner.get(&id()).await.unwrap().unwrap();
    assert_eq!(
        durable.context_ref.as_deref(),
        Some("newer-record-only-state")
    );
    let events = store.inner.events_since(&id(), 0).await.unwrap();
    assert_eq!(events.len(), 1);
    assert!(format!("{:?}", events[0].kind).contains("stale-event"));
    assert_eq!(durable.revision, 2);
}

#[tokio::test]
async fn a_public_resize_and_its_queued_flush_preserve_a_rival_record_field() {
    let store = Arc::new(FaultStore::default());
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let owner = user("resize-rebase-owner");
    let created = service
        .create_session(&owner, spec())
        .await
        .expect("session is created");
    await_settled(&service, &owner, &created.id).await;
    let lane = {
        let entries = service.entries.read().await;
        Arc::clone(&entries.get(&created.id).expect("live entry").event_log)
    };
    wait_for_lane_idle(&lane).await;
    let before = store
        .inner
        .get(&created.id)
        .await
        .unwrap()
        .expect("settled record exists");
    let before_tail = before.session.last_seq;
    let resized = TerminalGeometry::new(117, 39).expect("valid geometry");

    store.pause_next_write.store(true, Ordering::SeqCst);
    let resize_service = Arc::clone(&service);
    let resize_id = created.id.clone();
    let resize = tokio::spawn(async move {
        resize_service
            .resize_terminal(&owner, &resize_id, resized)
            .await
    });
    store.write_started.notified().await;

    let mut rival = store
        .inner
        .get(&created.id)
        .await
        .unwrap()
        .expect("rival reads the record");
    rival.context_ref = Some("rival-context".into());
    store
        .inner
        .commit_session_idempotent("rival-during-resize", &rival, &[])
        .await
        .expect("the rival record-only change commits first");
    store.release_write.notify_one();

    resize
        .await
        .expect("resize task completes")
        .expect("public resize completes after its durable barrier");
    wait_for_lane_idle(&lane).await;

    let durable = store.inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.session.geometry, resized);
    assert_eq!(durable.context_ref.as_deref(), Some("rival-context"));
    let events = store
        .inner
        .events_since(&created.id, before_tail)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].seq, before_tail + 1);
    assert!(matches!(
        &events[0].kind,
        crate::event::StoredEventKind::Known(EventKind::TerminalResized {
            columns: 117,
            rows: 39
        })
    ));
    assert_eq!(durable.session.last_seq, before_tail + 1);
}

#[tokio::test]
async fn a_rejected_resize_returns_failure_without_changing_the_runtime() {
    let store = Arc::new(FaultStore::default());
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let owner = user("resize-conflict-owner");
    let created = service
        .create_session(&owner, spec())
        .await
        .expect("session is created");
    await_settled(&service, &owner, &created.id).await;
    let lane = {
        let entries = service.entries.read().await;
        Arc::clone(&entries.get(&created.id).expect("live entry").event_log)
    };
    wait_for_lane_idle(&lane).await;
    let before = store
        .inner
        .get(&created.id)
        .await
        .unwrap()
        .expect("settled record exists");
    let before_tail = before.session.last_seq;
    let local_geometry = TerminalGeometry::new(118, 40).expect("valid geometry");
    let rival_geometry = TerminalGeometry::new(119, 41).expect("valid geometry");
    let before_screen = service
        .screen_snapshot(&owner, &created.id)
        .await
        .expect("screen is readable");
    let (input, mut runtime_input) = tokio::sync::mpsc::channel(1);
    service
        .entries
        .write()
        .await
        .get_mut(&created.id)
        .expect("live entry")
        .input = Some(input);

    store.pause_next_write.store(true, Ordering::SeqCst);
    let resize_service = Arc::clone(&service);
    let resize_id = created.id.clone();
    let resize_owner = owner.clone();
    let resize = tokio::spawn(async move {
        resize_service
            .resize_terminal(&resize_owner, &resize_id, local_geometry)
            .await
    });
    store.write_started.notified().await;

    let mut rival = store
        .inner
        .get(&created.id)
        .await
        .unwrap()
        .expect("rival reads the record");
    rival.session.geometry = rival_geometry;
    store
        .inner
        .commit_session_idempotent("rival-resize", &rival, &[])
        .await
        .expect("the rival geometry commits first");
    store.release_write.notify_one();

    let error = resize
        .await
        .expect("resize task completes")
        .expect_err("the rejected typed resize is reported");
    assert!(matches!(error, crate::CoreError::RuntimeUnavailable(_)));
    wait_for_lane_idle(&lane).await;

    let durable = store.inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.session.geometry, rival_geometry);
    assert_eq!(durable.session.last_seq, before_tail);
    assert!(store
        .inner
        .events_since(&created.id, before_tail)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        service
            .entries
            .read()
            .await
            .get(&created.id)
            .expect("live entry")
            .session
            .geometry,
        before.session.geometry,
        "a rejected proposal never becomes live state"
    );
    assert_eq!(
        service
            .screen_snapshot(&owner, &created.id)
            .await
            .expect("screen remains readable")
            .geometry,
        before_screen.geometry,
        "a rejected proposal never resizes the local screen"
    );
    assert!(
        matches!(
            runtime_input.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ),
        "a rejected proposal is never forwarded to the runtime"
    );
}

#[tokio::test]
async fn stale_runtime_loss_cannot_clear_a_replacement_runtime() {
    let mut base = record();
    base.runtime_id = Some("runtime-old".into());
    let mut stale = base.clone();
    stale.runtime_id = None;
    stale.session.recovery_state = RecoveryState::Recovering;
    let mut newer = base.clone();
    newer.runtime_id = Some("runtime-new".into());

    let (error, durable, events) = collide_with_newer_record(
        base,
        stale,
        Event {
            session_id: id(),
            seq: 0,
            kind: EventKind::RuntimeLost {
                runtime_id: "runtime-old".into(),
                cause: None,
            },
            appended_at: String::new(),
        },
        newer,
    )
    .await;

    assert!(matches!(
        error,
        StoreError::RevisionConflict {
            expected: 0,
            actual: 1
        }
    ));
    assert_eq!(durable.runtime_id.as_deref(), Some("runtime-new"));
    assert_eq!(durable.session.recovery_state, RecoveryState::Normal);
    assert!(events.is_empty());
}

#[tokio::test]
async fn runtime_loss_rebases_when_the_observed_runtime_is_still_current() {
    let store = Arc::new(FaultStore::default());
    let mut base = record();
    base.runtime_id = Some("runtime-old".into());
    store.inner.upsert(&base).await.expect("seed record");
    store.pause_next_write.store(true, Ordering::SeqCst);
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));

    let task_service = Arc::clone(&service);
    let mut stale = base.clone();
    stale.runtime_id = None;
    stale.session.recovery_state = RecoveryState::Recovering;
    let stale_write = tokio::spawn(async move {
        task_service
            .persist_durably(
                &mut stale,
                vec![Event {
                    session_id: id(),
                    seq: 0,
                    kind: EventKind::RuntimeLost {
                        runtime_id: "runtime-old".into(),
                        cause: None,
                    },
                    appended_at: String::new(),
                }],
            )
            .await
    });
    store.write_started.notified().await;
    let mut newer = base;
    newer.context_ref = Some("newer-unrelated-state".into());
    store
        .inner
        .commit_session_idempotent("newer-unrelated-record", &newer, &[])
        .await
        .expect("the unrelated record update commits first");
    store.release_write.notify_one();

    stale_write
        .await
        .expect("the stale writer task completes")
        .expect("the runtime identity still satisfies the loss precondition");
    let durable = store.inner.get(&id()).await.unwrap().unwrap();
    assert!(durable.runtime_id.is_none());
    assert_eq!(durable.session.recovery_state, RecoveryState::Recovering);
    assert_eq!(
        durable.context_ref.as_deref(),
        Some("newer-unrelated-state")
    );
    let events = store.inner.events_since(&id(), 0).await.unwrap();
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0].kind,
        crate::event::StoredEventKind::Known(EventKind::RuntimeLost { runtime_id, .. })
            if runtime_id == "runtime-old"
    ));
}

#[tokio::test]
async fn stale_status_change_cannot_restore_a_newer_lifecycle_state() {
    let base = record();
    let mut stale = base.clone();
    stale.session.status = SessionStatus::Running;
    let mut newer = base.clone();
    newer.session.status = SessionStatus::Idle;
    newer.session.stop_reason = Some(StopReason::UserTerminated);
    newer.session.recovery_state = RecoveryState::Idle;

    let (error, durable, events) = collide_with_newer_record(
        base,
        stale,
        Event {
            session_id: id(),
            seq: 0,
            kind: EventKind::StatusChanged {
                status: SessionStatus::Running,
                stop_reason: None,
                detail: None,
                capability: None,
            },
            appended_at: String::new(),
        },
        newer,
    )
    .await;

    assert!(matches!(error, StoreError::RevisionConflict { .. }));
    assert_eq!(durable.session.status, SessionStatus::Idle);
    assert_eq!(
        durable.session.stop_reason,
        Some(StopReason::UserTerminated)
    );
    assert_eq!(durable.session.recovery_state, RecoveryState::Idle);
    assert!(events.is_empty());
}

#[tokio::test]
async fn stale_plane_change_cannot_erase_a_newer_runtime_decision() {
    let mut base = record();
    base.session.options.mode = SessionMode::Terminal;
    base.runtime_id = Some("runtime-old".into());
    let mut stale = base.clone();
    stale.session.options.mode = SessionMode::Conversation;
    stale.runtime_id = None;
    stale.session.rotations = 1;
    let mut newer = base.clone();
    newer.runtime_id = Some("runtime-new".into());
    newer.session.runtime_version = Some("runtime-version-new".into());

    let (error, durable, events) = collide_with_newer_record(
        base,
        stale,
        Event {
            session_id: id(),
            seq: 0,
            kind: EventKind::PlaneChanged {
                from: SessionMode::Terminal,
                to: SessionMode::Conversation,
                snapshot_at: "2026-08-24T01:00:00Z".into(),
            },
            appended_at: String::new(),
        },
        newer,
    )
    .await;

    assert!(matches!(error, StoreError::RevisionConflict { .. }));
    assert_eq!(durable.session.options.mode, SessionMode::Terminal);
    assert_eq!(durable.runtime_id.as_deref(), Some("runtime-new"));
    assert_eq!(
        durable.session.runtime_version.as_deref(),
        Some("runtime-version-new")
    );
    assert_eq!(durable.session.rotations, 0);
    assert!(events.is_empty());
}

#[tokio::test]
async fn two_services_keep_both_semantic_writes_after_a_revision_collision() {
    let store = Arc::new(FakeStore::default());
    let base = record();
    store.upsert(&base).await.expect("seed record");
    let first_service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let second_service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let mut first_record = base.clone();
    let mut second_record = base;

    let (first, second) = tokio::join!(
        first_service.persist_durably(&mut first_record, vec![said(0, "from-first-service")]),
        second_service.persist_durably(&mut second_record, vec![said(0, "from-second-service")]),
    );

    first.expect("first service commits");
    second.expect("second service rebases and commits");
    let events = store.events_since(&id(), 0).await.unwrap();
    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        [1, 2]
    );
    let rendered = format!("{:?}", events);
    assert!(rendered.contains("from-first-service"));
    assert!(rendered.contains("from-second-service"));
    let durable = store.get(&id()).await.unwrap().unwrap();
    assert_eq!(durable.revision, 2);
    assert_eq!(durable.session.last_seq, 2);
}

#[tokio::test]
async fn a_newer_lane_revision_cannot_authorize_a_stale_store_snapshot() {
    let store = Arc::new(FakeStore::default());
    let base = record();
    store.upsert(&base).await.expect("seed record");
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));

    let mut newer = base.clone();
    newer.context_ref = Some("newer-state".into());
    service
        .persist_durably(&mut newer, Vec::new())
        .await
        .expect("the first snapshot advances the lane revision");

    let mut stale = base;
    stale.context_ref = Some("stale-state".into());
    assert!(matches!(
        service.persist_durably(&mut stale, Vec::new()).await,
        Err(StoreError::RevisionConflict {
            expected: 0,
            actual: 1
        })
    ));
    let durable = store.get(&id()).await.unwrap().unwrap();
    assert_eq!(durable.context_ref.as_deref(), Some("newer-state"));
}

#[tokio::test]
async fn observing_only_a_revision_never_authorizes_an_inheriting_snapshot() {
    let store = Arc::new(FakeStore::default());
    let base = record();
    store.upsert(&base).await.expect("seed record");
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let lane = service.event_logs.lane(&id());
    lane.observe_record(&base);
    wait_for_lane_idle(&lane).await;

    let mut rival = base.clone();
    rival.context_ref = Some("rival-state".into());
    let rival_revision = store
        .commit_session_idempotent("rival-before-observation", &rival, &[])
        .await
        .expect("rival commits");
    lane.observe_revision(rival_revision);

    let event = lane.enqueue_commit(
        super::CommitMode::UpdateInheriting,
        base.clone(),
        vec![said(0, "after-revision-observation")],
    );
    let barrier = lane.enqueue_commit(super::CommitMode::UpdateInheriting, base, Vec::new());
    event
        .complete
        .await
        .expect("worker remains")
        .expect("event rebases from the record, not the revision");
    barrier
        .complete
        .await
        .expect("worker remains")
        .expect("record-only successor preserves the rebase");

    let durable = store.get(&id()).await.unwrap().unwrap();
    assert_eq!(durable.context_ref.as_deref(), Some("rival-state"));
    let events = store.events_since(&id(), 0).await.unwrap();
    assert_eq!(events.len(), 1);
    assert!(format!("{:?}", events[0].kind).contains("after-revision-observation"));
}

#[tokio::test]
async fn an_exact_store_snapshot_does_not_redefine_the_live_snapshot_lineage() {
    let store = Arc::new(FakeStore::default());
    let base = record();
    store.upsert(&base).await.expect("seed record");
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let lane = service.event_logs.lane(&id());
    lane.observe_record(&base);
    wait_for_lane_idle(&lane).await;

    let mut exact = base.clone();
    exact.context_ref = Some("exact-record-state".into());
    lane.enqueue_commit(super::CommitMode::UpdateExact, exact, Vec::new())
        .complete
        .await
        .expect("worker remains")
        .expect("exact record commits");
    lane.enqueue_commit(super::CommitMode::UpdateInheriting, base, Vec::new())
        .complete
        .await
        .expect("worker remains")
        .expect("unchanged live state rebases onto the exact record");

    let durable = store.get(&id()).await.unwrap().unwrap();
    assert_eq!(durable.context_ref.as_deref(), Some("exact-record-state"));
}

#[tokio::test]
async fn a_queued_delete_cannot_be_followed_by_an_inheriting_resurrection() {
    let store = Arc::new(FakeStore::default());
    let base = record();
    store.upsert(&base).await.expect("seed record");
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let lane = service.event_logs.lane(&id());
    lane.observe_record(&base);
    wait_for_lane_idle(&lane).await;

    let deletion = lane.enqueue_delete();
    let successor = lane.enqueue_commit(
        super::CommitMode::UpdateInheriting,
        base,
        vec![said(0, "must-not-resurrect")],
    );
    deletion
        .await
        .expect("delete worker remains")
        .expect("delete commits");
    assert!(matches!(
        successor
            .complete
            .await
            .expect("write worker remains")
            .expect_err("the deleted record cannot be inherited"),
        StoreError::SessionDeleted { id: deleted } if deleted == id()
    ));
    assert!(store.get(&id()).await.unwrap().is_none());
    assert!(store.events_since(&id(), 0).await.unwrap().is_empty());
}

#[tokio::test]
async fn an_old_receipt_never_replays_its_record_over_a_newer_record_only_write() {
    let store = FakeStore::default();
    let base = record();
    store.upsert(&base).await.expect("seed record");

    let mut event_record = base.clone();
    event_record.session.last_seq = 1;
    let first_revision = store
        .commit_session_idempotent("write-a", &event_record, &[said(1, "event-a")])
        .await
        .expect("event transaction");
    assert_eq!(first_revision, 1);

    let mut newer = event_record.clone();
    newer.revision = first_revision;
    newer.context_ref = Some("newer-record-only-state".into());
    let second_revision = store
        .commit_session_idempotent("write-b", &newer, &[])
        .await
        .expect("empty record transaction is real work");
    assert_eq!(second_revision, 2);

    assert_eq!(
        store
            .commit_session_idempotent("write-a", &event_record, &[said(1, "event-a")])
            .await
            .expect("old receipt is idempotent"),
        1
    );
    let durable = store.get(&id()).await.unwrap().unwrap();
    assert_eq!(durable.revision, 2);
    assert_eq!(
        durable.context_ref.as_deref(),
        Some("newer-record-only-state")
    );
    assert_eq!(store.events_since(&id(), 0).await.unwrap().len(), 1);
}

#[tokio::test]
async fn an_empty_idempotent_commit_updates_the_fake_like_mongo() {
    let store = FakeStore::default();
    let mut update = record();
    store.upsert(&update).await.expect("seed record");
    update.context_ref = Some("record-only".into());

    let revision = store
        .commit_session_idempotent("empty-write", &update, &[])
        .await
        .expect("empty events do not mean empty transaction");

    assert_eq!(revision, 1);
    let durable = store.get(&id()).await.unwrap().unwrap();
    assert_eq!(durable.context_ref.as_deref(), Some("record-only"));
    assert_eq!(durable.revision, 1);
}

#[tokio::test]
async fn a_legacy_record_tail_remains_the_floor_for_new_atomic_writes() {
    let store = FakeStore::default();
    store
        .append_events(&[said(1, "one"), said(2, "two")])
        .await
        .expect("seed the physical event tail");
    let mut legacy = record();
    legacy.session.last_seq = 3;
    store
        .upsert(&legacy)
        .await
        .expect("seed a tail advertised by the old two-step writer");

    assert!(matches!(
        store.append_events(&[said(3, "reserved")]).await,
        Err(StoreError::SequenceTaken {
            expected: 4,
            got: 3
        })
    ));

    let mut update = legacy;
    update.session.last_seq = 4;
    store
        .commit_session_idempotent("after-legacy-gap", &update, &[said(4, "four")])
        .await
        .expect("new history continues after the already-advertised cursor");

    let sequences: Vec<u64> = store
        .events_since(&id(), 0)
        .await
        .unwrap()
        .into_iter()
        .map(|event| event.seq)
        .collect();
    assert_eq!(sequences, [1, 2, 4]);
    let mut durable = store.get(&id()).await.unwrap().unwrap();
    assert_eq!(durable.session.last_seq, 4);

    durable.session.last_seq = 2;
    store
        .commit_session_idempotent("stale-record-only-tail", &durable, &[])
        .await
        .expect("record-only writes preserve an event tail already ahead of them");
    let mut durable = store.get(&id()).await.unwrap().unwrap();
    assert_eq!(durable.session.last_seq, 4);

    durable.session.last_seq = 5;
    assert!(matches!(
        store
            .commit_session_idempotent("invented-record-only-tail", &durable, &[])
            .await,
        Err(StoreError::Malformed(_))
    ));
}

#[tokio::test]
async fn a_terminal_fake_record_cannot_be_written_back_as_live() {
    let store = FakeStore::default();
    let mut terminal = record();
    terminal.session.status = SessionStatus::Terminated;
    store.upsert(&terminal).await.expect("seed terminal record");

    let mut stale = terminal.clone();
    stale.session.status = SessionStatus::Provisioning;
    assert!(matches!(
        store
            .commit_session_idempotent("stale-live-write", &stale, &[])
            .await,
        Err(StoreError::SessionTerminated { id: refused }) if refused == id()
    ));
    assert_eq!(
        store.get(&id()).await.unwrap().unwrap().session.status,
        SessionStatus::Terminated
    );
}

#[tokio::test]
async fn the_canonical_fake_refuses_lower_and_internal_gap_batches_atomically() {
    let store = FakeStore::default();
    store
        .append_events(&[said(1, "one")])
        .await
        .expect("first event");
    assert!(matches!(
        store.append_events(&[said(1, "duplicate")]).await,
        Err(StoreError::SequenceTaken {
            expected: 2,
            got: 1
        })
    ));
    assert!(matches!(
        store
            .append_events(&[said(2, "two"), said(4, "four")])
            .await,
        Err(StoreError::NonContiguousAppend {
            expected: 3,
            got: 4
        })
    ));
    assert_eq!(store.events_since(&id(), 0).await.unwrap().len(), 1);
}

#[tokio::test]
async fn dead_lane_keys_are_pruned_on_the_next_lookup() {
    let store = Arc::new(FakeStore::default());
    let service = service_over(store, Arc::new(FakeAdapter::default()));
    for index in 0..128 {
        let lane = service
            .event_logs
            .lane(&SessionId(format!("sesn_ephemeral_{index}")));
        assert_eq!(lane.queued(), 0);
        drop(lane);
    }
    let last = service
        .event_logs
        .lane(&SessionId("sesn_registry_probe".into()));
    assert_eq!(service.event_logs.registry_len(), 1);
    drop(last);
}
