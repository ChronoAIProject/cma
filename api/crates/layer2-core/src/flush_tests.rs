//! FIFO ownership and cross-service deletion regressions.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Notify};

use crate::error::CoreError;
use crate::event::{Event, EventKind, StoredEvent};
use crate::ids::{SessionId, SubjectId};
use crate::session::{RecoveryState, Session, SessionStatus};
use crate::store::{Page, SessionListQuery, SessionRecord, SessionStore, StoreError};
use crate::test_support::{await_settled, service_over, user, FakeAdapter, FakeStore};
use crate::{
    AgentOptions, ConversationLink, ConversationOpeningLease, ConversationOpeningLeaseRequest,
    ConversationWriteAuthority, KernelId, Reserved, SessionLease, TerminalGeometry,
};

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
    async fn ping(&self) -> Result<(), StoreError> {
        self.inner.ping().await
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
        self.inner.get(id).await
    }

    async fn list_for(
        &self,
        owner: &SubjectId,
        query: &SessionListQuery,
    ) -> Result<Page<SessionRecord>, StoreError> {
        self.inner.list_for(owner, query).await
    }

    async fn reserved_capacity(&self) -> Result<Reserved, StoreError> {
        self.inner.reserved_capacity().await
    }

    async fn list_all(&self, query: &SessionListQuery) -> Result<Page<SessionRecord>, StoreError> {
        self.inner.list_all(query).await
    }

    async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
        self.inner.delete(id).await
    }

    async fn append_events(&self, events: &[Event]) -> Result<(), StoreError> {
        self.inner.append_events(events).await
    }

    async fn acquire_conversation_opening(
        &self,
        request: &ConversationOpeningLeaseRequest,
        session_lease: Option<&SessionLease>,
    ) -> Result<ConversationOpeningLease, StoreError> {
        self.inner
            .acquire_conversation_opening(request, session_lease)
            .await
    }

    async fn renew_conversation_opening(
        &self,
        opening: &ConversationOpeningLease,
        session_lease: Option<&SessionLease>,
        ttl: Duration,
    ) -> Result<(), StoreError> {
        self.inner
            .renew_conversation_opening(opening, session_lease, ttl)
            .await
    }

    async fn publish_conversation_link(
        &self,
        opening: &ConversationOpeningLease,
        session_lease: Option<&SessionLease>,
    ) -> Result<ConversationLink, StoreError> {
        self.inner
            .publish_conversation_link(opening, session_lease)
            .await
    }

    async fn release_conversation_opening(
        &self,
        opening: &ConversationOpeningLease,
    ) -> Result<(), StoreError> {
        self.inner.release_conversation_opening(opening).await
    }

    async fn validate_conversation_link(&self, link: &ConversationLink) -> Result<(), StoreError> {
        self.inner.validate_conversation_link(link).await
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

    async fn commit_conversation_idempotent(
        &self,
        write_id: &str,
        authority: &ConversationWriteAuthority,
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
            .commit_conversation_idempotent(write_id, authority, record, events)
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

async fn settled_live_session(
    store: Arc<PairingStore>,
    adapter: Arc<FakeAdapter>,
) -> (
    Arc<crate::service::SessionService>,
    crate::authz::Caller,
    SessionId,
    String,
    SessionRecord,
) {
    let service = service_over(store.clone(), adapter);
    let owner = user("subject-live-lifecycle");
    let created = service
        .create_session(&owner, crate::test_support::spec())
        .await
        .expect("session is created");
    await_settled(&service, &owner, &created.id).await;
    let lane = service.event_logs.lane(&created.id);
    tokio::time::timeout(Duration::from_secs(2), async {
        while lane.queued() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("provisioning writes settle");
    let before = store
        .inner
        .get(&created.id)
        .await
        .unwrap()
        .expect("settled record exists");
    let runtime = before.runtime_id.clone().expect("runtime is durable");
    store.calls.store(0, Ordering::SeqCst);
    (service, owner, created.id, runtime, before)
}

async fn assert_live_lifecycle_conflict(terminate: bool) {
    let store = Arc::new(PairingStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let (service, owner, id, runtime, before) =
        settled_live_session(store.clone(), adapter.clone()).await;
    let lane = service.event_logs.lane(&id);

    store.arm();
    let operation_service = Arc::clone(&service);
    let operation_owner = owner.clone();
    let operation_id = id.clone();
    let operation = tokio::spawn(async move {
        if terminate {
            operation_service
                .terminate_session(&operation_owner, &operation_id)
                .await
        } else {
            operation_service
                .stop_session(&operation_owner, &operation_id)
                .await
        }
    });
    store.wait_until_held().await;

    {
        let entries = service.entries.read().await;
        let entry = entries.get(&id).expect("live entry remains installed");
        assert_eq!(entry.session.status, SessionStatus::Ready);
        assert_eq!(
            entry.runtime.as_ref().map(|handle| &handle.0),
            Some(&runtime)
        );
        assert!(entry.input.is_some());
    }

    let mut rival = store.inner.get(&id).await.unwrap().unwrap();
    rival.session.status = SessionStatus::Running;
    rival.session.stop_reason = None;
    rival.session.recovery_state = RecoveryState::Normal;
    let rival_seq = rival.session.last_seq + 1;
    rival.session.last_seq = rival_seq;
    store
        .inner
        .commit_session_idempotent(
            if terminate {
                "rival-during-live-terminate"
            } else {
                "rival-during-live-stop"
            },
            &rival,
            &[Event {
                session_id: id.clone(),
                seq: rival_seq,
                kind: EventKind::StatusChanged {
                    status: SessionStatus::Running,
                    stop_reason: None,
                    detail: None,
                    capability: None,
                },
                appended_at: String::new(),
            }],
        )
        .await
        .expect("the rival lifecycle decision commits first");
    store.release.notify_one();

    assert!(matches!(
        operation.await.expect("lifecycle task completes"),
        Err(CoreError::RuntimeUnavailable(_))
    ));
    tokio::time::timeout(Duration::from_secs(2), async {
        while lane.queued() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the rejected lifecycle proposal leaves the lane");

    let durable = store.inner.get(&id).await.unwrap().unwrap();
    assert_eq!(durable.session.status, SessionStatus::Running);
    assert_eq!(durable.session.last_seq, rival_seq);
    assert_eq!(durable.runtime_id.as_deref(), Some(runtime.as_str()));
    let replay = store
        .inner
        .events_since(&id, before.session.last_seq)
        .await
        .unwrap();
    assert_eq!(replay.len(), 1);
    assert!(matches!(
        &replay[0].kind,
        crate::event::StoredEventKind::Known(EventKind::StatusChanged {
            status: SessionStatus::Running,
            ..
        })
    ));
    let entries = service.entries.read().await;
    let entry = entries.get(&id).expect("live entry survives rejection");
    assert_eq!(entry.session.status, SessionStatus::Ready);
    assert_eq!(entry.session.last_seq, before.session.last_seq);
    assert_eq!(
        entry.runtime.as_ref().map(|handle| &handle.0),
        Some(&runtime)
    );
    assert!(entry.input.is_some());
    drop(entries);
    assert!(adapter.terminated.lock().await.is_empty());
    assert!(adapter.running.lock().await.contains_key(&runtime));
}

async fn assert_live_lifecycle_unavailable_stops(terminate: bool) {
    let store = Arc::new(PairingStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let (service, owner, id, runtime, before) =
        settled_live_session(store.clone(), adapter.clone()).await;
    let lane = service.event_logs.lane(&id);
    store.fail_next(4);

    let result = if terminate {
        service.terminate_session(&owner, &id).await
    } else {
        service.stop_session(&owner, &id).await
    };
    assert!(matches!(result, Err(CoreError::RuntimeUnavailable(_))));
    tokio::time::timeout(Duration::from_secs(2), async {
        while lane.queued() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the reported lifecycle failure leaves the lane");
    assert_eq!(store.calls.load(Ordering::SeqCst), 4);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        store.calls.load(Ordering::SeqCst),
        4,
        "no detached retry continues after the fourth observed failure"
    );

    let durable = store.inner.get(&id).await.unwrap().unwrap();
    assert_eq!(durable.session.status, before.session.status);
    assert_eq!(
        durable.session.recovery_state,
        before.session.recovery_state
    );
    assert_eq!(durable.session.stop_reason, before.session.stop_reason);
    assert_eq!(durable.session.last_seq, before.session.last_seq);
    assert_eq!(durable.runtime_id.as_deref(), Some(runtime.as_str()));
    assert!(store
        .inner
        .events_since(&id, before.session.last_seq)
        .await
        .unwrap()
        .is_empty());
    let entries = service.entries.read().await;
    let entry = entries.get(&id).expect("live entry survives store failure");
    assert_eq!(entry.session.status, SessionStatus::Ready);
    assert_eq!(entry.session.last_seq, before.session.last_seq);
    assert_eq!(
        entry.runtime.as_ref().map(|handle| &handle.0),
        Some(&runtime)
    );
    assert!(entry.input.is_some());
    drop(entries);
    assert!(adapter.terminated.lock().await.is_empty());
    assert!(adapter.running.lock().await.contains_key(&runtime));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_live_stop_conflict_preserves_the_rival_state_and_runtime() {
    assert_live_lifecycle_conflict(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_live_terminate_conflict_preserves_the_rival_state_and_runtime() {
    assert_live_lifecycle_conflict(true).await;
}

#[tokio::test]
async fn a_live_stop_ends_after_four_observed_store_failures() {
    assert_live_lifecycle_unavailable_stops(false).await;
}

#[tokio::test]
async fn a_live_terminate_ends_after_four_observed_store_failures() {
    assert_live_lifecycle_unavailable_stops(true).await;
}

async fn assert_cancelled_live_lifecycle_finishes(terminate: bool) {
    let store = Arc::new(PairingStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let (service, owner, id, runtime, before) =
        settled_live_session(store.clone(), adapter.clone()).await;
    let lane = service.event_logs.lane(&id);

    store.arm();
    let operation = tokio::spawn({
        let service = Arc::clone(&service);
        let owner = owner.clone();
        let id = id.clone();
        async move {
            if terminate {
                service.terminate_session(&owner, &id).await
            } else {
                service.stop_session(&owner, &id).await
            }
        }
    });
    store.wait_until_held().await;
    operation.abort();
    let _ = operation.await;
    store.release.notify_one();

    let expected = if terminate {
        SessionStatus::Terminated
    } else {
        SessionStatus::Idle
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let durable = store.inner.get(&id).await.unwrap().unwrap();
            let torn_down = adapter.terminated.lock().await.contains(&runtime);
            let installed = if terminate {
                service.entries.read().await.get(&id).is_some_and(|entry| {
                    entry.session.status == expected && entry.runtime.is_none()
                })
            } else {
                !service.entries.read().await.contains_key(&id)
            };
            if durable.session.status == expected
                && durable.runtime_id.is_none()
                && torn_down
                && installed
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the cancelled live lifecycle continuation finishes");
    tokio::time::timeout(Duration::from_secs(2), async {
        while lane.queued() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the retained lifecycle lane drains");

    let durable = store.inner.get(&id).await.unwrap().unwrap();
    assert_eq!(durable.session.last_seq, before.session.last_seq + 1);
    assert!(!adapter.running.lock().await.contains_key(&runtime));
    assert_eq!(
        adapter
            .terminated
            .lock()
            .await
            .iter()
            .filter(|handle| *handle == &runtime)
            .count(),
        1
    );

    service
        .terminate_session(&owner, &id)
        .await
        .expect("a later termination observes or advances the retained result");
    assert_eq!(
        adapter
            .terminated
            .lock()
            .await
            .iter()
            .filter(|handle| *handle == &runtime)
            .count(),
        1,
        "later lifecycle work does not tear the same runtime down twice"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_live_stop_retains_entry_removal_and_runtime_teardown() {
    assert_cancelled_live_lifecycle_finishes(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_live_terminate_retains_entry_installation_and_runtime_teardown() {
    assert_cancelled_live_lifecycle_finishes(true).await;
}

async fn assert_cancelled_no_entry_lifecycle_finishes(terminate: bool) {
    let store = Arc::new(PairingStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let mut base = base_record();
    let runtime = if terminate {
        "runtime-cancelled-no-entry-terminate"
    } else {
        "runtime-cancelled-no-entry-stop"
    }
    .to_string();
    base.runtime_id = Some(runtime.clone());
    store.inner.seed(&base).await.expect("seed record");
    adapter
        .running
        .lock()
        .await
        .insert(runtime.clone(), Some(id().0));
    let service = service_over(store.clone(), adapter.clone());
    let lane = service.event_logs.lane(&id());

    store.arm();
    let operation = tokio::spawn({
        let service = Arc::clone(&service);
        async move {
            if terminate {
                service
                    .terminate_session(&user("subject-fifo"), &id())
                    .await
            } else {
                service.stop_session(&user("subject-fifo"), &id()).await
            }
        }
    });
    store.wait_until_held().await;
    operation.abort();
    let _ = operation.await;
    store.release.notify_one();

    let expected = if terminate {
        SessionStatus::Terminated
    } else {
        SessionStatus::Idle
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let durable = store.inner.get(&id()).await.unwrap().unwrap();
            if durable.session.status == expected
                && durable.runtime_id.is_none()
                && adapter.terminated.lock().await.contains(&runtime)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the cancelled no-entry lifecycle continuation finishes");
    tokio::time::timeout(Duration::from_secs(2), async {
        while lane.queued() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the retained no-entry lifecycle lane drains");

    let durable = store.inner.get(&id()).await.unwrap().unwrap();
    assert_eq!(durable.session.last_seq, base.session.last_seq + 1);
    assert!(!service.entries.read().await.contains_key(&id()));
    assert!(!adapter.running.lock().await.contains_key(&runtime));
    assert_eq!(
        adapter
            .terminated
            .lock()
            .await
            .iter()
            .filter(|handle| *handle == &runtime)
            .count(),
        1
    );

    service
        .terminate_session(&user("subject-fifo"), &id())
        .await
        .expect("later lifecycle work observes or advances the retained result");
    assert_eq!(
        adapter
            .terminated
            .lock()
            .await
            .iter()
            .filter(|handle| *handle == &runtime)
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_no_entry_stop_retains_its_original_runtime_handle() {
    assert_cancelled_no_entry_lifecycle_finishes(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_no_entry_terminate_retains_its_original_runtime_handle() {
    assert_cancelled_no_entry_lifecycle_finishes(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_cursor_waits_for_a_lower_runtime_event_racing_send_prompt() {
    let store = Arc::new(PairingStore::default());
    let adapter = Arc::new(FakeAdapter {
        capabilities: crate::conversation_support::conversing(),
        ..FakeAdapter::default()
    });
    let profiles =
        crate::test_support::environments_defaulting_to(crate::environment::AgentConfiguration {
            kernel: crate::test_support::kernel(),
            mode: crate::options::SessionMode::Conversation,
            ..Default::default()
        });
    let service = crate::test_support::service_over_with_environments(
        store.clone(),
        adapter.clone(),
        profiles,
    );
    let owner = user("subject-public-cursor");
    let created = service
        .create_session(&owner, crate::test_support::spec())
        .await
        .expect("conversation session is created");
    crate::test_support::await_settled(&service, &owner, &created.id).await;
    let lane = service.event_logs.lane(&created.id);
    tokio::time::timeout(Duration::from_secs(2), async {
        while lane.queued() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("provisioning writes settle");
    let cursor = service
        .events_since(&owner, &created.id, None)
        .await
        .expect("initial history is public")
        .last()
        .expect("creation has history")
        .seq;

    store.arm();
    let updates = adapter
        .conversation
        .lock()
        .await
        .as_ref()
        .expect("conversation is open")
        .updates
        .clone();
    updates
        .send(layer3_adapter::ConversationUpdate::Said(
            layer3_adapter::SessionUpdate::AvailableCommandsUpdate(
                crate::conversation_support::offering(&[("inspect", "Inspect the workspace")]),
            ),
        ))
        .await
        .expect("runtime update reaches its folder");
    store.wait_until_held().await;

    let prompt_service = Arc::clone(&service);
    let prompt_owner = owner.clone();
    let prompt_id = created.id.clone();
    let prompt = tokio::spawn(async move {
        prompt_service
            .send_prompt(&prompt_owner, &prompt_id, "what changed?".into())
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        // The running folder event is held inside the store. Its record barrier
        // and the prompt's atomic three-event unit must both be queued before
        // the read; a lower threshold could pass before `send_prompt` had
        // enqueued any history.
        while lane.queued() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("prompt commits queue behind the held runtime update");
    assert!(
        !prompt.is_finished(),
        "prompt admission waits for its atomic event unit to commit"
    );

    assert!(
        service
            .events_since(&owner, &created.id, Some(cursor))
            .await
            .expect("public cursor remains readable")
            .is_empty(),
        "the public reader cannot observe a higher prompt cursor while the lower runtime event is held"
    );

    store.release.notify_one();
    prompt
        .await
        .expect("prompt task completes")
        .expect("prompt is recorded after the runtime update");
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let replay = service
                .events_since(&owner, &created.id, Some(cursor))
                .await
                .expect("public replay remains readable");
            if replay.len() >= 4 {
                break replay;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("both public workflows become durable")
    .windows(2)
    .for_each(|pair| assert_eq!(pair[1].seq, pair[0].seq + 1));
}

#[tokio::test]
async fn two_queued_writers_keep_each_event_with_its_own_record_snapshot() {
    let store = Arc::new(PairingStore::default());
    let base = base_record();
    store.inner.seed(&base).await.expect("seed record");
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
    store.seed(&base).await.expect("seed record");
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
    store.inner.seed(&base).await.expect("seed record");
    let mut unrelated = base.clone();
    unrelated.session.id = SessionId("sesn_unrelated_progress".into());
    store
        .inner
        .seed(&unrelated)
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

async fn assert_failed_no_entry_lifecycle_stops_permanently(terminate: bool) {
    let store = Arc::new(PairingStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let mut base = base_record();
    let handle = if terminate {
        "runtime-failed-no-entry-terminate"
    } else {
        "runtime-failed-no-entry-stop"
    }
    .to_string();
    base.runtime_id = Some(handle.clone());
    store.inner.seed(&base).await.expect("seed record");
    adapter
        .running
        .lock()
        .await
        .insert(handle.clone(), Some(id().0));
    store.fail_next(4);
    let service = service_over(store.clone(), adapter.clone());

    let result = if terminate {
        service
            .terminate_session(&user("subject-fifo"), &id())
            .await
    } else {
        service.stop_session(&user("subject-fifo"), &id()).await
    };
    assert!(result.is_err(), "the fourth store failure is reported");
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
async fn a_failed_no_entry_stop_cannot_commit_later_or_tear_down_the_runtime() {
    assert_failed_no_entry_lifecycle_stops_permanently(false).await;
}

#[tokio::test]
async fn a_failed_no_entry_terminate_cannot_commit_later_or_tear_down_the_runtime() {
    assert_failed_no_entry_lifecycle_stops_permanently(true).await;
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
    store.seed(&stale).await.expect("seed live record");
    adapter.running.lock().await.insert(handle, Some(id().0));

    let service = service_over(store.clone(), adapter);
    let mut terminal = stale.clone();
    terminal.session.status = SessionStatus::Terminated;
    terminal.runtime_id = None;
    store
        .seed(&terminal)
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
