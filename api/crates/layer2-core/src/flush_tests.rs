//! FIFO ownership and cross-service deletion regressions.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Notify};

use crate::error::CoreError;
use crate::event::{Event, EventKind, StoredEvent};
use crate::ids::{SessionId, SubjectId};
use crate::session::{RecoveryState, Session, SessionStatus};
use crate::store::{SessionRecord, SessionStore, StoreError};
use crate::test_support::{service_over, user, FakeAdapter, FakeStore};
use crate::{AgentOptions, KernelId, Reserved, TerminalGeometry};

fn id() -> SessionId {
    SessionId("sesn_fifo_writers".into())
}

fn base_record() -> SessionRecord {
    SessionRecord::new(Session {
        id: id(),
        owner: SubjectId("subject-fifo".into()),
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

fn event(seq: u64, writer: &str) -> Event {
    Event {
        session_id: id(),
        seq,
        kind: EventKind::MessageCommitted {
            content: writer.into(),
        },
        appended_at: String::new(),
    }
}

#[derive(Default)]
struct PairingStore {
    inner: Arc<FakeStore>,
    delay_next: AtomicBool,
    entered: Notify,
    release: Notify,
    calls: AtomicUsize,
    failures: AtomicUsize,
    committed: Mutex<Vec<(String, String)>>,
}

impl PairingStore {
    fn arm(&self) {
        assert!(!self.delay_next.swap(true, Ordering::SeqCst));
    }

    fn fail_next(&self, count: usize) {
        self.failures.store(count, Ordering::SeqCst);
    }

    async fn wait_until_held(&self) {
        tokio::time::timeout(Duration::from_secs(2), self.entered.notified())
            .await
            .expect("first writer reached the store");
    }

    async fn before_commit(&self) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.delay_next.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
    }

    async fn remember(&self, record: &SessionRecord, events: &[Event]) {
        let record_owner = record.context_ref.clone().unwrap_or_default();
        let event_owner = events
            .first()
            .map(|event| format!("{:?}", event.kind))
            .unwrap_or_default();
        self.committed
            .lock()
            .await
            .push((record_owner, event_owner));
    }
}

#[async_trait::async_trait]
impl SessionStore for PairingStore {
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
        self.before_commit().await;
        if self
            .failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            return Err(StoreError::Unavailable("delayed test failure".into()));
        }
        let revision = self
            .inner
            .commit_session_idempotent(write_id, record, events)
            .await?;
        self.remember(record, events).await;
        Ok(revision)
    }

    async fn create_session_idempotent(
        &self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError> {
        self.inner
            .create_session_idempotent(write_id, record, events)
            .await
    }

    async fn events_since(
        &self,
        id: &SessionId,
        after: u64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        self.inner.events_since(id, after).await
    }
}

#[tokio::test]
async fn two_queued_writers_keep_each_event_with_its_own_record_snapshot() {
    let store = Arc::new(PairingStore::default());
    let base = base_record();
    store.inner.upsert(&base).await.expect("seed record");
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));

    let mut first_record = base.clone();
    first_record.context_ref = Some("record-a".into());
    store.arm();
    let first_service = Arc::clone(&service);
    let first = tokio::spawn(async move {
        first_service
            .persist_durably(&mut first_record, vec![event(0, "event-a")])
            .await
    });
    store.wait_until_held().await;

    let mut second_record = base;
    // This writer is queued as the intended successor to writer A. The lane
    // may order it, but must not replace its declared predecessor revision.
    second_record.revision = 1;
    second_record.context_ref = Some("record-b".into());
    let second_service = Arc::clone(&service);
    let second = tokio::spawn(async move {
        second_service
            .persist_durably(&mut second_record, vec![event(0, "event-b")])
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let lane = service.event_logs.lane(&id());
            if lane.queued() >= 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("second writer queued behind the first");
    second.abort();
    let _ = second.await;

    assert_eq!(
        store.calls.load(Ordering::SeqCst),
        1,
        "the second write cannot pass the held first write"
    );
    assert!(
        store.inner.events_since(&id(), 0).await.unwrap().is_empty(),
        "a cursor cannot advance while sequence one is held"
    );
    store.release.notify_one();
    first
        .await
        .expect("first waiter task")
        .expect("first durable write");

    tokio::time::timeout(Duration::from_secs(2), async {
        while store.committed.lock().await.len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancelled second waiter did not cancel its write");

    let committed = store.committed.lock().await.clone();
    assert_eq!(committed.len(), 2);
    assert_eq!(committed[0].0, "record-a");
    assert!(committed[0].1.contains("event-a"));
    assert_eq!(committed[1].0, "record-b");
    assert!(committed[1].1.contains("event-b"));
    let replay: Vec<u64> = store
        .inner
        .events_since(&id(), 0)
        .await
        .unwrap()
        .into_iter()
        .map(|event| event.seq)
        .collect();
    assert_eq!(replay, [1, 2]);
    let durable = store.inner.get(&id()).await.unwrap().unwrap();
    assert_eq!(durable.context_ref.as_deref(), Some("record-b"));
    assert_eq!(durable.revision, 2);
}

#[tokio::test]
async fn deletion_in_another_service_prevents_stale_resurrection() {
    let store = Arc::new(FakeStore::default());
    let base = base_record();
    store.upsert(&base).await.expect("seed record");
    let stale_service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let deleting_service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let mut stale = base;
    stale.context_ref = Some("stale-replica".into());

    deleting_service
        .delete_durably(&id())
        .await
        .expect("other replica deleted the session durably");
    let result = stale_service
        .persist_durably(&mut stale, vec![event(0, "must-not-resurrect")])
        .await;

    assert!(matches!(
        result,
        Err(StoreError::SessionDeleted { id: deleted }) if deleted == id()
    ));
    assert!(store.get(&id()).await.unwrap().is_none());
    assert!(store.events_since(&id(), 0).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_no_entry_stop_blocks_only_its_session_while_the_store_call_is_pending() {
    let store = Arc::new(PairingStore::default());
    let base = base_record();
    store.inner.upsert(&base).await.expect("seed record");
    let mut unrelated = base.clone();
    unrelated.session.id = SessionId("sesn_unrelated_progress".into());
    store
        .inner
        .upsert(&unrelated)
        .await
        .expect("seed unrelated record");
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));

    store.arm();
    let stopping_service = Arc::clone(&service);
    let stopping = tokio::spawn(async move {
        stopping_service
            .stop_session(&user("subject-fifo"), &id())
            .await
    });
    store.wait_until_held().await;

    assert!(
        tokio::time::timeout(
            Duration::from_millis(50),
            service.lifecycle_gates.gate(&id()).lock_owned(),
        )
        .await
        .is_err(),
        "the same session cannot install while its stop is becoming durable"
    );
    drop(
        tokio::time::timeout(Duration::from_millis(50), service.entries.write())
            .await
            .expect("the service-wide entries lock stays available"),
    );

    let unrelated_id = unrelated.session.id.clone();
    service
        .persist_durably(
            &mut unrelated,
            vec![Event {
                session_id: unrelated_id.clone(),
                seq: 0,
                kind: EventKind::MessageCommitted {
                    content: "unrelated session progressed".into(),
                },
                appended_at: String::new(),
            }],
        )
        .await
        .expect("an unrelated session commits through its own lane");
    assert_eq!(
        store
            .events_since(&unrelated_id, 0)
            .await
            .expect("unrelated history reads")
            .len(),
        1
    );

    store.release.notify_one();
    stopping
        .await
        .expect("stop task")
        .expect("stop becomes durable");
    assert_eq!(
        store
            .inner
            .get(&id())
            .await
            .unwrap()
            .unwrap()
            .session
            .status,
        SessionStatus::Idle
    );
}

#[tokio::test]
async fn a_failed_no_entry_stop_cannot_commit_later_or_tear_down_the_runtime() {
    let store = Arc::new(PairingStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let mut base = base_record();
    let handle = "runtime-that-must-stay-live".to_string();
    base.runtime_id = Some(handle.clone());
    store.inner.upsert(&base).await.expect("seed record");
    adapter
        .running
        .lock()
        .await
        .insert(handle.clone(), Some(id().0));
    store.fail_next(4);
    let service = service_over(store.clone(), adapter.clone());

    assert!(
        service
            .stop_session(&user("subject-fifo"), &id())
            .await
            .is_err(),
        "the fourth store failure is reported"
    );
    assert_eq!(store.calls.load(Ordering::SeqCst), 4);

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        store.calls.load(Ordering::SeqCst),
        4,
        "the failed lifecycle operation has no retained writer"
    );
    let durable = store.inner.get(&id()).await.unwrap().unwrap();
    assert_eq!(durable.session.status, SessionStatus::Ready);
    assert_eq!(durable.runtime_id.as_deref(), Some(handle.as_str()));
    assert!(adapter.terminated.lock().await.is_empty());
    assert!(adapter.running.lock().await.contains_key(&handle));
    assert!(store.inner.events_since(&id(), 0).await.unwrap().is_empty());
}

#[tokio::test]
async fn completed_lifecycle_gates_do_not_accumulate_session_keys() {
    let service = service_over(
        Arc::new(FakeStore::default()),
        Arc::new(FakeAdapter::default()),
    );

    for number in 0..128 {
        let gate = service
            .lifecycle_gates
            .gate(&SessionId(format!("sesn_gate_{number}")));
        drop(gate.lock().await);
        drop(gate);
    }

    assert!(
        service.lifecycle_gates.registry_len() <= 1,
        "dead weak gates are pruned on each lookup"
    );
}

#[tokio::test]
async fn readoption_cannot_install_from_a_snapshot_that_has_since_terminated() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let mut stale = base_record();
    let handle = "runtime-stale-readoption".to_string();
    stale.runtime_id = Some(handle.clone());
    store.upsert(&stale).await.expect("seed live record");
    adapter.running.lock().await.insert(handle, Some(id().0));

    let service = service_over(store.clone(), adapter);
    let mut terminal = stale.clone();
    terminal.session.status = SessionStatus::Terminated;
    terminal.runtime_id = None;
    store
        .upsert(&terminal)
        .await
        .expect("the stale read is superseded durably");

    assert!(matches!(
        service.readopt(&stale).await,
        Err(CoreError::SessionTerminated)
    ));
    assert!(
        !service.entries.read().await.contains_key(&id()),
        "a completed termination must not be followed by a stale local entry"
    );
}
