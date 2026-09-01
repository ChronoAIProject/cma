//! Adversarial persistence recovery and store-contract tests.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

use crate::commit_fault_support::CommitFaultStore;
use crate::coordination::{
    ConversationOpeningLeaseRequest, ConversationWriteAuthority, SessionBoundary,
    SessionLeaseRequest, SessionOperation,
};
use crate::event::{Event, EventKind, StoredEvent};
use crate::ids::{SessionId, SubjectId};
use crate::options::SessionMode;
use crate::session::{RecoveryState, Session, SessionStatus, StopReason};
use crate::store::{
    Page, PendingHandover, PendingHandoverTarget, SessionListQuery, SessionRecord, SessionStore,
    StoreError,
};
use crate::test_support::{
    await_settled, service_over, spec, terminal_input_channel, user, FakeAdapter, FakeStore,
};
use crate::{AgentOptions, KernelId, Reserved, TerminalGeometry};

fn id() -> SessionId {
    SessionId("sesn_log_recovery".into())
}

fn record() -> SessionRecord {
    SessionRecord::new(Session {
        chat: None,
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

#[test]
fn trigger_prompt_execution_allowlist_matches_the_adapter_contract_exactly() {
    let durable_activity = [
        EventKind::MessageCommitted {
            content: "working".into(),
        },
        EventKind::ToolCallStarted {
            call_id: "call-1".into(),
            title: "read".into(),
            kind: "read".into(),
            arguments: None,
        },
        EventKind::ToolCallUpdated {
            call_id: "call-1".into(),
            status: "completed".into(),
            result: None,
        },
        EventKind::ApprovalRequested {
            request_id: "approval-1".into(),
            action: Some("write".into()),
            detail: None,
            locator: None,
        },
    ];
    let classified: Vec<_> = durable_activity
        .iter()
        .filter_map(super::initial_prompt_execution_evidence)
        .collect();
    assert_eq!(
        classified.as_slice(),
        &layer3_adapter::TurnScopedEvidence::ALL
    );

    let initialization = [
        EventKind::AvailableCommandsSet {
            commands: Vec::new(),
        },
        EventKind::ModeChanged {
            mode: "agent".into(),
        },
        EventKind::ConfigOptionsSet {
            options: Vec::new(),
        },
        EventKind::UsageReported {
            input_tokens: 0,
            output_tokens: 0,
            context_used: 0,
            context_window: Some(200_000),
        },
        EventKind::SignInRequired {
            url: "https://example.invalid/device".into(),
            code: Some("ABCD".into()),
            message: None,
        },
    ];
    for event in &initialization {
        assert_eq!(
            super::initial_prompt_execution_evidence(event),
            None,
            "initialization event became execution evidence: {event:?}"
        );
    }
    assert_eq!(
        super::initial_prompt_execution_evidence(&EventKind::OwnerMessage {
            content: "CMA wrote this before dispatch".into(),
        }),
        None,
        "CMA-authored input is not recipient acknowledgement"
    );
}

fn pending_plane_handover(claim: &str) -> PendingHandover {
    PendingHandover {
        claim: claim.into(),
        snapshot_ref: format!("sessions/{}/handover.snapshot", id().0),
        snapshot_at: "2026-08-24T01:00:00Z".into(),
        trigger_recovery: None,
        rotations_from: 0,
        rotations_to: 1,
        target: PendingHandoverTarget::Plane {
            from_mode: SessionMode::Terminal,
            mode: SessionMode::Conversation,
            capabilities: Default::default(),
        },
    }
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

#[tokio::test]
async fn external_marker_occupying_an_inflight_reservation_survives_rejection_rewind() {
    let store = Arc::new(FakeStore::default());
    let mut seeded = record();
    seeded.runtime_id = Some("runtime-original".into());
    store.seed(&seeded).await.expect("seed session");

    let opening = store
        .acquire_conversation_opening(
            &ConversationOpeningLeaseRequest {
                session_id: id(),
                holder: "old-conversation-holder".into(),
                token: "old-conversation-opening".into(),
                runtime_id: "runtime-original".into(),
                runtime_generation: 0,
                runtime_candidate_token: None,
                ttl: Duration::from_secs(30),
            },
            None,
        )
        .await
        .expect("acquire old conversation opening");
    let old_link = store
        .publish_conversation_link(&opening, None)
        .await
        .expect("publish old conversation link");
    let stale_record = store
        .get(&id())
        .await
        .expect("read stale record")
        .expect("session remains");

    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let lane = service.event_logs.lane(&id());
    lane.pause_next_operation_after_pop();
    let stale = lane.enqueue_conversation_commit(
        super::CommitMode::UpdateExact,
        stale_record,
        vec![said(0, "stale-reserved-write")],
        ConversationWriteAuthority::Active(old_link),
    );
    tokio::time::timeout(Duration::from_secs(2), lane.operation_popped.notified())
        .await
        .expect("the stale write is paused after reserving sequence one");

    let authority = store
        .runtime_authority(&id())
        .await
        .expect("read old runtime authority")
        .expect("runtime authority exists");
    let request = SessionLeaseRequest::new(
        id(),
        "replacement-holder",
        SessionBoundary {
            status: SessionStatus::Ready,
            authority,
        },
        SessionOperation::Handover,
        Duration::from_secs(30),
    );
    let mut lease = store
        .acquire_session_lease(&request)
        .await
        .expect("acquire replacement lease");
    let reservation = store
        .reserve_runtime_generation(&lease, "replacement-reservation")
        .await
        .expect("reserve replacement generation");
    let candidate = store
        .register_runtime_candidate(&lease, &reservation, "runtime-replacement")
        .await
        .expect("register replacement runtime");
    let switched = store
        .switch_runtime_authority(&mut lease, &candidate)
        .await
        .expect("commit replacement authority and marker");
    assert_eq!(switched.runtime_started_seq, 1);

    lane.observe_tail(switched.runtime_started_seq);
    assert_eq!(
        service.sequences.current(&id()),
        Some(switched.runtime_started_seq),
        "the marker becomes the allocator's durable floor"
    );
    lane.release_popped_operation.notify_one();

    let rejection = stale
        .complete
        .committed()
        .await
        .expect_err("the old conversation link is fenced by the switch");
    assert!(matches!(
        rejection,
        StoreError::ConversationLinkChanged { .. }
    ));
    assert_eq!(
        service.sequences.current(&id()),
        Some(switched.runtime_started_seq),
        "rejection cannot return the marker's occupied sequence"
    );

    let successor_record = store
        .get(&id())
        .await
        .expect("read switched record")
        .expect("session remains after switch");
    let successor = lane.enqueue_commit(
        super::CommitMode::UpdateExact,
        successor_record,
        vec![said(0, "readiness-successor")],
    );
    successor
        .complete
        .committed()
        .await
        .expect("the readiness successor follows the marker");
    lane.fence().await;

    let events = store
        .events_since(&id(), 0)
        .await
        .expect("read final history");
    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        [1, 2],
        "the marker and readiness successor remain contiguous"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                matches!(
                    &event.kind,
                    crate::event::StoredEventKind::Known(EventKind::RuntimeStarted { .. })
                )
            })
            .count(),
        1,
        "the interleaving writes exactly one runtime marker"
    );
}

async fn marker_concurrency_fixture() -> (Arc<FakeStore>, SessionRecord, ConversationWriteAuthority)
{
    let store = Arc::new(FakeStore::default());
    let mut seeded = record();
    seeded.runtime_id = Some("runtime-original".into());
    store.seed(&seeded).await.expect("seed session");

    let opening = store
        .acquire_conversation_opening(
            &ConversationOpeningLeaseRequest {
                session_id: id(),
                holder: "old-conversation-holder".into(),
                token: "old-conversation-opening".into(),
                runtime_id: "runtime-original".into(),
                runtime_generation: 0,
                runtime_candidate_token: None,
                ttl: Duration::from_secs(30),
            },
            None,
        )
        .await
        .expect("acquire old conversation opening");
    let old_link = store
        .publish_conversation_link(&opening, None)
        .await
        .expect("publish old conversation link");
    let stale_record = store
        .get(&id())
        .await
        .expect("read stale record")
        .expect("session remains");

    (
        store,
        stale_record,
        ConversationWriteAuthority::Active(old_link),
    )
}

async fn commit_replacement_runtime_marker(store: &FakeStore) -> u64 {
    let authority = store
        .runtime_authority(&id())
        .await
        .expect("read old runtime authority")
        .expect("runtime authority exists");
    let request = SessionLeaseRequest::new(
        id(),
        "replacement-holder",
        SessionBoundary {
            status: SessionStatus::Ready,
            authority,
        },
        SessionOperation::Handover,
        Duration::from_secs(30),
    );
    let mut lease = store
        .acquire_session_lease(&request)
        .await
        .expect("acquire replacement lease");
    let reservation = store
        .reserve_runtime_generation(&lease, "replacement-reservation")
        .await
        .expect("reserve replacement generation");
    let candidate = store
        .register_runtime_candidate(&lease, &reservation, "runtime-replacement")
        .await
        .expect("register replacement runtime");
    store
        .switch_runtime_authority(&mut lease, &candidate)
        .await
        .expect("commit replacement authority and marker")
        .runtime_started_seq
}

#[tokio::test]
async fn external_marker_occupying_a_batch_prefix_shifts_prequeued_successors() {
    let (store, stale_record, stale_authority) = marker_concurrency_fixture().await;
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let lane = service.event_logs.lane(&id());
    lane.pause_next_operation_after_pop();

    let stale = lane.enqueue_conversation_commit(
        super::CommitMode::UpdateExact,
        stale_record.clone(),
        vec![said(0, "stale-batch-one"), said(0, "stale-batch-two")],
        stale_authority,
    );
    tokio::time::timeout(Duration::from_secs(2), lane.operation_popped.notified())
        .await
        .expect("the stale two-event batch is paused after reserving one and two");

    let first_successor = lane.enqueue_commit(
        super::CommitMode::UpdateExact,
        stale_record.clone(),
        vec![
            said(0, "first-prequeued-successor-one"),
            said(0, "first-prequeued-successor-two"),
        ],
    );
    let second_successor = lane.enqueue_commit(
        super::CommitMode::UpdateExact,
        stale_record,
        vec![said(0, "second-prequeued-successor")],
    );
    assert_eq!(
        service.sequences.current(&id()),
        Some(5),
        "the stale batch and both successors reserve one contiguous local range"
    );

    let marker_seq = commit_replacement_runtime_marker(&store).await;
    assert_eq!(marker_seq, 1);
    lane.observe_tail(marker_seq);
    lane.release_popped_operation.notify_one();

    assert!(matches!(
        stale
            .complete
            .committed()
            .await
            .expect_err("the switch fences the stale conversation batch"),
        StoreError::ConversationLinkChanged { .. }
    ));
    let first_record = first_successor
        .complete
        .committed()
        .await
        .expect("the first prequeued successor follows the marker");
    let second_record = second_successor
        .complete
        .committed()
        .await
        .expect("the second prequeued successor follows the first");
    lane.fence().await;

    assert_eq!(first_record.session.last_seq, 3);
    assert_eq!(second_record.session.last_seq, 4);
    assert_eq!(service.sequences.current(&id()), Some(4));
    let events = store
        .events_since(&id(), 0)
        .await
        .expect("read shifted marker history");
    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
    assert!(matches!(
        &events[0].kind,
        crate::event::StoredEventKind::Known(EventKind::RuntimeStarted { .. })
    ));
    let messages = events
        .iter()
        .filter_map(|event| match &event.kind {
            crate::event::StoredEventKind::Known(EventKind::MessageCommitted { content }) => {
                Some(content.as_str())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        messages,
        [
            "first-prequeued-successor-one",
            "first-prequeued-successor-two",
            "second-prequeued-successor",
        ],
        "only the marker and shifted successors become durable"
    );
}

#[tokio::test]
async fn external_marker_inside_a_prequeued_successor_resequences_the_whole_queue() {
    let (store, stale_record, stale_authority) = marker_concurrency_fixture().await;
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let lane = service.event_logs.lane(&id());
    lane.pause_next_operation_after_pop();

    let stale = lane.enqueue_conversation_commit(
        super::CommitMode::UpdateExact,
        stale_record.clone(),
        vec![said(0, "stale-before-external-history")],
        stale_authority,
    );
    tokio::time::timeout(Duration::from_secs(2), lane.operation_popped.notified())
        .await
        .expect("the stale write is paused after reserving sequence one");

    let first_successor = lane.enqueue_commit(
        super::CommitMode::UpdateExact,
        stale_record.clone(),
        vec![
            said(0, "first-overlapping-successor-one"),
            said(0, "first-overlapping-successor-two"),
        ],
    );
    let second_successor = lane.enqueue_commit(
        super::CommitMode::UpdateExact,
        stale_record.clone(),
        vec![
            said(0, "second-overlapping-successor-one"),
            said(0, "second-overlapping-successor-two"),
        ],
    );
    assert_eq!(service.sequences.current(&id()), Some(5));

    let mut external_record = stale_record;
    external_record.session.last_seq = 2;
    store
        .commit_session_idempotent(
            "external-history-before-marker",
            &external_record,
            &[
                said(1, "external-history-one"),
                said(2, "external-history-two"),
            ],
        )
        .await
        .expect("an external writer occupies the first two reservations");
    let marker_seq = commit_replacement_runtime_marker(&store).await;
    assert_eq!(
        marker_seq, 3,
        "the marker occupies the tail of the first successor's original reservation"
    );
    lane.observe_tail(marker_seq);
    lane.release_popped_operation.notify_one();

    assert!(matches!(
        stale
            .complete
            .committed()
            .await
            .expect_err("the switch fences the stale conversation write"),
        StoreError::ConversationLinkChanged { .. }
    ));
    let first_record = first_successor
        .complete
        .committed()
        .await
        .expect("the overlapping successor rebases after the marker");
    let second_record = second_successor
        .complete
        .committed()
        .await
        .expect("the later successor moves with the same queue rebase");
    lane.fence().await;

    assert_eq!(first_record.session.last_seq, 5);
    assert_eq!(second_record.session.last_seq, 7);
    assert_eq!(service.sequences.current(&id()), Some(7));
    let events = store
        .events_since(&id(), 0)
        .await
        .expect("read resequenced marker history");
    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5, 6, 7]
    );
    assert!(matches!(
        &events[2].kind,
        crate::event::StoredEventKind::Known(EventKind::RuntimeStarted { .. })
    ));
    let messages = events
        .iter()
        .filter_map(|event| match &event.kind {
            crate::event::StoredEventKind::Known(EventKind::MessageCommitted { content }) => {
                Some(content.as_str())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        messages,
        [
            "external-history-one",
            "external-history-two",
            "first-overlapping-successor-one",
            "first-overlapping-successor-two",
            "second-overlapping-successor-one",
            "second-overlapping-successor-two",
        ],
        "the stale reservation is absent and every successor remains FIFO"
    );
}

struct FaultStore {
    inner: Arc<FakeStore>,
    failures: AtomicUsize,
    failures_after_first_write: AtomicUsize,
    calls: AtomicUsize,
    lose_next_response: AtomicBool,
    pause_next_write: AtomicBool,
    write_started: Notify,
    release_write: Notify,
    delete_calls: AtomicUsize,
    delete_failures: AtomicUsize,
    pause_delete_before_call: AtomicUsize,
    pause_delete_after_call: AtomicUsize,
    delete_started: Notify,
    release_delete: Notify,
}

impl Default for FaultStore {
    fn default() -> Self {
        let inner = Arc::new(FakeStore::default());
        inner.disable_coordination();
        Self {
            inner,
            failures: AtomicUsize::default(),
            failures_after_first_write: AtomicUsize::default(),
            calls: AtomicUsize::default(),
            lose_next_response: AtomicBool::default(),
            pause_next_write: AtomicBool::default(),
            write_started: Notify::new(),
            release_write: Notify::new(),
            delete_calls: AtomicUsize::default(),
            delete_failures: AtomicUsize::default(),
            pause_delete_before_call: AtomicUsize::default(),
            pause_delete_after_call: AtomicUsize::default(),
            delete_started: Notify::new(),
            release_delete: Notify::new(),
        }
    }
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
        let call = self.delete_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self
            .pause_delete_before_call
            .compare_exchange(call, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            self.delete_started.notify_one();
            self.release_delete.notified().await;
        }
        if Self::consume_failure(&self.delete_failures) {
            return Err(StoreError::Unavailable(
                "transient test delete failure".into(),
            ));
        }
        self.inner.delete(id).await?;
        if self
            .pause_delete_after_call
            .compare_exchange(call, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            self.delete_started.notify_one();
            self.release_delete.notified().await;
        }
        Ok(())
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

fn current_thread_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime builds")
}

fn is_runtime_drop_probe(_record: &SessionRecord, events: &[Event]) -> bool {
    events.iter().any(|event| {
        matches!(
            &event.kind,
            EventKind::MessageCommitted { content }
                if content.starts_with("runtime-drop-first")
        )
    })
}

fn is_runtime_drop_resize(_record: &SessionRecord, events: &[Event]) -> bool {
    events.iter().any(|event| {
        matches!(
            event.kind,
            EventKind::TerminalResized {
                columns: 133,
                rows: 47
            }
        )
    })
}

fn is_runtime_drop_rejection(record: &SessionRecord, events: &[Event]) -> bool {
    events.is_empty() && record.context_ref.as_deref() == Some("stale-retained-rejection")
}

fn is_final_rejection_probe(_record: &SessionRecord, events: &[Event]) -> bool {
    events.iter().any(|event| {
        matches!(event.kind, EventKind::TerminalResized { .. })
            || matches!(
                &event.kind,
                EventKind::MessageCommitted { content }
                    if content == "late-after-final-rejection"
            )
    })
}

fn enqueue_test_fence(lane: &Arc<super::SessionLog>) -> super::LaneReceiver<()> {
    let (complete, receiver) = tokio::sync::oneshot::channel();
    lane.enqueue(super::PendingOperation::Fence {
        complete: Some(complete),
    });
    super::LaneReceiver::new(receiver, Arc::clone(lane))
}

async fn collide_with_newer_record(
    base: SessionRecord,
    mut stale: SessionRecord,
    event: Event,
    newer: SessionRecord,
) -> (StoreError, SessionRecord, Vec<StoredEvent>) {
    let store = Arc::new(FaultStore::default());
    store.inner.seed(&base).await.expect("seed record");
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
        rotation_precondition: None,
        terminal_lifecycle_repair: false,
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
        rotation_precondition: None,
        terminal_lifecycle_repair: false,
    };
    let after_status = status_change
        .apply(&intended, after_message)
        .expect("the semantic event owns the status change");
    assert_eq!(after_status.session.status, SessionStatus::Running);
    assert_eq!(after_status.context_ref.as_deref(), Some("rival-state"));
}

#[test]
fn an_informational_event_cannot_resurrect_runtime_authority_from_its_snapshot() {
    let before = record();
    let mut intended = before.clone();
    intended.runtime_id = Some("runtime-outgoing".into());
    let fields = super::RecordFields::for_commit(
        &before,
        &intended,
        &[said(1, "queued-before-authority-switch")],
    );
    let change = super::RecordChange {
        before: Box::new(before),
        fields,
        rotation_precondition: None,
        terminal_lifecycle_repair: false,
    };
    let mut durable = record();
    durable.runtime_id = Some("runtime-current".into());
    durable.revision = 1;

    let rebased = change
        .apply(&intended, durable)
        .expect("history does not own runtime authority");
    assert_eq!(rebased.runtime_id.as_deref(), Some("runtime-current"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sequence_allocation_cannot_be_inverted_before_lane_insertion() {
    let store = Arc::new(FakeStore::default());
    let base = record();
    store.seed(&base).await.expect("seed record");
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
            None,
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
    store.inner.seed(&seeded).await.expect("seed record");
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
async fn dropping_a_success_delivered_before_poll_runs_its_cleanup() {
    let cleaned = Arc::new(AtomicBool::new(false));
    let cleanup_flag = Arc::clone(&cleaned);
    let delivery = super::CommitDelivery::retained(
        "delivered-output",
        "retained-ownership",
        move |output, ownership| {
            assert_eq!(output, "delivered-output");
            assert_eq!(ownership, "retained-ownership");
            cleanup_flag.store(true, Ordering::SeqCst);
        },
    );
    let (send, receive) = tokio::sync::oneshot::channel();

    assert!(
        send.send(delivery).is_ok(),
        "the success is placed in the unpolled receiver"
    );
    std::thread::spawn(move || {
        assert!(
            tokio::runtime::Handle::try_current().is_err(),
            "the regression must drop outside every entered Tokio runtime"
        );
        drop(receive);
    })
    .join()
    .expect("the outside-runtime receiver drop does not panic");

    tokio::time::timeout(Duration::from_secs(1), async {
        while !cleaned.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("dropping an unpolled success launches its retained cleanup");
}

#[test]
fn a_restartable_write_survives_its_worker_runtime_stopping_before_the_first_poll() {
    let origin = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("origin runtime builds");
    let store = Arc::new(FakeStore::default());
    let seeded = record();
    origin.block_on(store.seed(&seeded)).expect("seed record");
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let lane = service.event_logs.lane(&id());
    let mut commit = {
        let _entered = origin.enter();
        lane.enqueue_restartable_commit(seeded, vec![said(0, "survives-an-unpolled-worker")])
    };

    drop(origin);
    assert!(
        !lane
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .running,
        "dropping an unpolled worker makes its queued lane restartable"
    );

    let replacement = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("replacement runtime builds");
    replacement.block_on(async {
        commit.restart();
        let committed = tokio::time::timeout(Duration::from_secs(2), commit.committed())
            .await
            .expect("the replacement runtime restarts the queued write")
            .expect("the retained write commits");
        assert_eq!(committed.session.last_seq, 1);
        let events = store.events_since(&id(), 0).await.unwrap();
        assert_eq!(events.len(), 1);
        assert!(format!("{:?}", events[0].kind).contains("survives-an-unpolled-worker"));
    });
}

#[test]
fn a_retained_continuation_survives_runtime_stopping_before_any_worker_poll() {
    let origin = current_thread_runtime();
    let store = Arc::new(FakeStore::default());
    let seeded = record();
    origin.block_on(store.seed(&seeded)).expect("seed record");
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let lane = service.event_logs.lane(&id());
    let continued = Arc::new(AtomicBool::new(false));
    let continuation_flag = Arc::clone(&continued);

    let fence = {
        let _entered = origin.enter();
        let pending = service.persist_durably_with(
            seeded,
            vec![said(0, "retained-before-any-worker-poll")],
            move |_committed| async move {
                continuation_flag.store(true, Ordering::SeqCst);
                ((), ())
            },
            super::ignore_rejected,
            super::drop_undelivered,
        );
        let fence = enqueue_test_fence(&lane);
        drop(pending);
        fence
    };

    drop(origin);
    assert!(
        !lane
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .running,
        "dropping an unpolled worker releases its retained operation"
    );

    let replacement = current_thread_runtime();
    replacement.block_on(async {
        fence
            .await
            .expect("the prequeued fence restarts the retained lane");
        assert!(
            continued.load(Ordering::SeqCst),
            "the FIFO cannot reach its fence before the retained continuation"
        );
        wait_for_lane_idle(&lane).await;
    });

    let events = replacement.block_on(store.events_since(&id(), 0)).unwrap();
    assert_eq!(events.len(), 1);
    assert!(format!("{:?}", events[0].kind).contains("retained-before-any-worker-poll"));
}

#[test]
fn a_public_resize_continuation_and_lifecycle_survive_store_runtime_replacement() {
    let origin = current_thread_runtime();
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(
        Arc::clone(&inner),
        is_runtime_drop_resize,
    ));
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let owner = user("runtime-replaced-resize-owner");
    let created = origin.block_on(async {
        let created = service
            .create_session(&owner, spec())
            .await
            .expect("session is created");
        await_settled(&service, &owner, &created.id).await;
        created
    });
    let lane = origin.block_on(async {
        let entries = service.entries.read().await;
        Arc::clone(&entries.get(&created.id).expect("live entry").event_log)
    });
    origin.block_on(wait_for_lane_idle(&lane));
    let before = origin
        .block_on(inner.get(&created.id))
        .unwrap()
        .expect("settled record exists");
    let resized = TerminalGeometry::new(133, 47).expect("valid geometry");
    let lifecycle_gate = service.lifecycle_gates.gate(&created.id);
    store.pause_next();

    origin.block_on(async {
        let resize = tokio::spawn({
            let service = Arc::clone(&service);
            let owner = owner.clone();
            let id = created.id.clone();
            async move { service.resize_terminal(&owner, &id, resized).await }
        });
        store.wait_until_held().await;
        assert_eq!(
            inner
                .get(&created.id)
                .await
                .unwrap()
                .expect("the pre-resize record remains durable")
                .session
                .geometry,
            before.session.geometry,
            "the selected resize is still pending inside the store"
        );
        assert!(
            lifecycle_gate.try_lock().is_err(),
            "the public resize owns its lifecycle gate before runtime loss"
        );
        drop(resize);
    });
    let first_write_id = store
        .matched_write_ids()
        .into_iter()
        .next()
        .expect("the paused resize records its write identity");

    drop(origin);
    assert!(
        !lane
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .running,
        "the interrupted resize releases the lane for a replacement worker"
    );
    assert!(
        lifecycle_gate.try_lock().is_err(),
        "the queued retained operation, not the lost runtime, owns the lifecycle gate"
    );

    let replacement = current_thread_runtime();
    replacement.block_on(async {
        let (replacement_input, _received) = terminal_input_channel(2);
        service
            .entries
            .write()
            .await
            .get_mut(&created.id)
            .expect("the live entry survives the executor")
            .input = Some(replacement_input);
        let public_successor = TerminalGeometry::new(134, 48).expect("valid geometry");
        tokio::time::timeout(
            Duration::from_secs(2),
            service.resize_terminal(&owner, &created.id, public_successor),
        )
        .await
        .expect("a public lifecycle call restarts the retained lane")
        .expect("the public successor follows the recovered resize");
        assert_eq!(
            service
                .entries
                .read()
                .await
                .get(&created.id)
                .expect("live entry survives")
                .session
                .geometry,
            public_successor,
            "the public successor runs after the retained live continuation"
        );
        assert_eq!(
            service
                .screen_snapshot(&owner, &created.id)
                .await
                .expect("screen remains readable")
                .geometry,
            public_successor
        );
        wait_for_lane_idle(&lane).await;
    });

    let write_ids = store.matched_write_ids();
    assert_eq!(write_ids, [first_write_id.clone(), first_write_id]);
    let durable = replacement
        .block_on(inner.get(&created.id))
        .unwrap()
        .expect("resized record is durable");
    assert_eq!(durable.session.geometry.columns(), 134);
    assert_eq!(durable.session.geometry.rows(), 48);
    assert_eq!(durable.session.last_seq, before.session.last_seq + 2);
    let events = replacement
        .block_on(inner.events_since(&created.id, before.session.last_seq))
        .unwrap();
    assert_eq!(events.len(), 2);
    assert!(matches!(
        events[0].kind,
        crate::event::StoredEventKind::Known(EventKind::TerminalResized {
            columns: 133,
            rows: 47
        })
    ));
    assert!(matches!(
        events[1].kind,
        crate::event::StoredEventKind::Known(EventKind::TerminalResized {
            columns: 134,
            rows: 48
        })
    ));
    assert!(
        lifecycle_gate.try_lock().is_ok(),
        "undelivered success releases lifecycle ownership after continuation"
    );
}

#[test]
fn a_public_lifecycle_call_recovers_an_orphaned_polled_continuation() {
    let origin = current_thread_runtime();
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(
        Arc::clone(&inner),
        is_runtime_drop_probe,
    ));
    let seeded = record();
    origin.block_on(inner.seed(&seeded)).expect("seed record");
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let lifecycle_gate = service.lifecycle_gates.gate(&id());
    let continuation_started = Arc::new(Notify::new());
    let continued = Arc::new(AtomicUsize::new(0));

    let mut pending = None;
    origin.block_on(async {
        let lifecycle = service.lock_lifecycle(&id()).await;
        let started = Arc::clone(&continuation_started);
        let continuation_count = Arc::clone(&continued);
        pending = Some(service.persist_durably_with(
            seeded,
            vec![said(0, "runtime-drop-first-continuation-pending")],
            move |_committed| async move {
                let mut timer = Box::pin(tokio::time::sleep(Duration::from_secs(1)));
                std::future::poll_fn(|context| {
                    match std::future::Future::poll(timer.as_mut(), context) {
                        std::task::Poll::Pending => {
                            started.notify_one();
                            std::task::Poll::Ready(())
                        }
                        std::task::Poll::Ready(()) => {
                            panic!("the continuation timer must become pending before runtime loss")
                        }
                    }
                })
                .await;
                timer.await;
                continuation_count.fetch_add(1, Ordering::SeqCst);
                ((), lifecycle)
            },
            super::ignore_rejected,
            super::drop_undelivered,
        ));
        tokio::time::timeout(Duration::from_secs(2), continuation_started.notified())
            .await
            .expect("the durable continuation reaches its pending await");
        assert_eq!(inner.events_since(&id(), 0).await.unwrap().len(), 1);
    });
    let pending = pending.expect("the retained completion receiver is constructed");
    assert_eq!(store.matched_calls(), 1);
    assert_eq!(continued.load(Ordering::SeqCst), 0);
    drop(pending);

    drop(origin);
    assert!(
        lifecycle_gate.try_lock().is_err(),
        "the stored continuation future retains lifecycle ownership"
    );

    let replacement = current_thread_runtime();
    replacement.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(5),
            service.stop_session(&user("subject-log"), &id()),
        )
        .await
        .expect("a public lifecycle call restarts the orphaned retained lane")
        .expect("the public stop follows the recovered continuation");
        assert_eq!(
            continued.load(Ordering::SeqCst),
            1,
            "the same continuation future resumes exactly once before the public stop"
        );
    });

    assert_eq!(
        store.matched_calls(),
        1,
        "resuming post-commit work does not replay an already durable write"
    );
    assert_eq!(store.matched_write_ids().len(), 1);
    let events = replacement.block_on(inner.events_since(&id(), 0)).unwrap();
    assert_eq!(events.len(), 2);
    assert!(format!("{:?}", events[0].kind).contains("runtime-drop-first-continuation-pending"));
    assert!(
        lifecycle_gate.try_lock().is_ok(),
        "undelivered completion releases lifecycle ownership after continuation"
    );
    drop(
        service
            .event_logs
            .lane(&SessionId("sesn_retained_cycle_prune_probe".into())),
    );
    assert_eq!(
        service.event_logs.registry_len(),
        1,
        "completed retained work breaks its lane keepalive cycle"
    );
}

#[test]
fn a_public_lifecycle_call_recovers_an_orphaned_polled_rejection() {
    let origin = current_thread_runtime();
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(
        Arc::clone(&inner),
        is_runtime_drop_rejection,
    ));
    let mut stale = record();
    stale.context_ref = Some("stale-retained-rejection".into());
    origin.block_on(async {
        inner.seed(&stale).await.expect("seed record");
        let mut rival = stale.clone();
        rival.context_ref = Some("rival-before-retained-rejection".into());
        inner
            .commit_session_idempotent("rival-before-retained-rejection", &rival, &[])
            .await
            .expect("the rival advances the durable revision");
    });
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let lifecycle_gate = service.lifecycle_gates.gate(&id());
    let rejection_started = Arc::new(Notify::new());
    let release_rejection = Arc::new(Notify::new());
    let rejected = Arc::new(AtomicUsize::new(0));
    let continued = Arc::new(AtomicBool::new(false));

    let mut pending = None;
    origin.block_on(async {
        let lifecycle = service.lock_lifecycle(&id()).await;
        let continuation_flag = Arc::clone(&continued);
        let started = Arc::clone(&rejection_started);
        let release = Arc::clone(&release_rejection);
        let rejection_count = Arc::clone(&rejected);
        pending = Some(service.persist_durably_with(
            stale,
            Vec::new(),
            move |_committed| async move {
                continuation_flag.store(true, Ordering::SeqCst);
                ((), lifecycle)
            },
            move |_error| async move {
                started.notify_one();
                release.notified().await;
                rejection_count.fetch_add(1, Ordering::SeqCst);
            },
            super::drop_undelivered,
        ));
        tokio::time::timeout(Duration::from_secs(2), rejection_started.notified())
            .await
            .expect("the retained rejection reaches its pending await");
    });
    let pending = pending.expect("the retained rejection receiver is constructed");
    assert_eq!(rejected.load(Ordering::SeqCst), 0);
    assert!(!continued.load(Ordering::SeqCst));
    drop(pending);

    drop(origin);
    assert!(
        lifecycle_gate.try_lock().is_err(),
        "the retained but uncalled continuation still owns lifecycle state"
    );

    let replacement = current_thread_runtime();
    replacement.block_on(async {
        release_rejection.notify_one();
        tokio::time::timeout(
            Duration::from_secs(2),
            service.stop_session(&user("subject-log"), &id()),
        )
        .await
        .expect("a public lifecycle call restarts the orphaned retained lane")
        .expect("the public stop follows the recovered rejection");
        assert_eq!(
            rejected.load(Ordering::SeqCst),
            1,
            "the same rejection future resumes exactly once before the public stop"
        );
    });

    assert!(!continued.load(Ordering::SeqCst));
    assert_eq!(store.matched_calls(), 1);
    assert_eq!(store.matched_write_ids().len(), 1);
    assert_eq!(
        replacement
            .block_on(inner.get(&id()))
            .unwrap()
            .expect("the rival record remains durable")
            .context_ref
            .as_deref(),
        Some("rival-before-retained-rejection")
    );
    assert!(
        lifecycle_gate.try_lock().is_ok(),
        "final rejection releases lifecycle ownership after cleanup"
    );
}

#[test]
fn a_pre_enqueue_retained_rejection_survives_runtime_loss_and_precedes_its_successor() {
    let origin = current_thread_runtime();
    let store = Arc::new(FakeStore::default());
    let seeded = record();
    origin.block_on(store.seed(&seeded)).expect("seed record");
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let lane = service.event_logs.lane(&id());
    let lifecycle_gate = service.lifecycle_gates.gate(&id());
    let lifecycle = origin.block_on(service.lock_lifecycle(&id()));
    let continued = Arc::new(AtomicBool::new(false));
    let rejected = Arc::new(AtomicUsize::new(0));

    let successor = {
        let _entered = origin.enter();
        let continuation_flag = Arc::clone(&continued);
        let rejection_count = Arc::clone(&rejected);
        let (retained, pending) = super::retain_commit(
            id(),
            Arc::clone(&lane),
            move |_committed| async move {
                continuation_flag.store(true, Ordering::SeqCst);
                ((), lifecycle)
            },
            move |error| async move {
                assert!(matches!(error, StoreError::Malformed(_)));
                rejection_count.fetch_add(1, Ordering::SeqCst);
            },
            super::drop_undelivered,
        );
        let mut malformed = said(0, "retained-malformed-before-enqueue");
        malformed.session_id = SessionId("sesn_wrong_lane".into());
        lane.enqueue_retained_commit(
            super::CommitRequest {
                mode: super::CommitMode::UpdateExact,
                record: seeded.clone(),
                events: vec![malformed],
                semantics: super::CommitSemantics::default(),
                conversation_authority: None,
                chat_runtime_transition: None,
            },
            retained,
            |_| {},
        );
        let successor = lane.enqueue_commit(
            super::CommitMode::UpdateExact,
            seeded,
            vec![said(0, "successor-after-pre-enqueue-rejection")],
        );
        drop(pending);
        successor
    };

    drop(origin);
    assert!(
        lifecycle_gate.try_lock().is_err(),
        "the uncalled continuation owns lifecycle state across runtime loss"
    );

    let replacement = current_thread_runtime();
    replacement.block_on(async {
        successor
            .complete
            .committed()
            .await
            .expect("the successor restarts and follows retained rejection cleanup");
        assert_eq!(
            rejected.load(Ordering::SeqCst),
            1,
            "pre-enqueue rejection cleanup runs once before its FIFO successor"
        );
        wait_for_lane_idle(&lane).await;
    });

    assert!(!continued.load(Ordering::SeqCst));
    assert!(lifecycle_gate.try_lock().is_ok());
    let events = replacement.block_on(store.events_since(&id(), 0)).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].seq, 1);
    assert!(format!("{:?}", events[0].kind).contains("successor-after-pre-enqueue"));
}

#[test]
fn an_ordinary_write_survives_runtime_stopping_after_pop_before_store_entry() {
    let origin = current_thread_runtime();
    let inner = Arc::new(FakeStore::default());
    let seeded = record();
    origin.block_on(inner.seed(&seeded)).expect("seed record");
    let store = Arc::new(CommitFaultStore::new(
        Arc::clone(&inner),
        is_runtime_drop_probe,
    ));
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let lane = service.event_logs.lane(&id());
    lane.pause_next_operation_after_pop();

    let (first, successor) = origin.block_on(async {
        let first = lane.enqueue_commit(
            super::CommitMode::UpdateExact,
            seeded.clone(),
            vec![said(0, "runtime-drop-first-before-store")],
        );
        let successor = lane.enqueue_commit(
            super::CommitMode::UpdateExact,
            seeded,
            vec![said(0, "runtime-drop-successor-before-store")],
        );
        tokio::time::timeout(Duration::from_secs(2), lane.operation_popped.notified())
            .await
            .expect("the worker pops the first operation");
        (first, successor)
    });
    assert_eq!(store.matched_calls(), 0, "the store was not entered");

    drop(origin);
    {
        let state = lane.state.lock().unwrap_or_else(|error| error.into_inner());
        assert!(!state.running, "the cancelled worker releases the lane");
        assert_eq!(state.queued.len(), 2, "the first operation is restored");
    }

    let replacement = current_thread_runtime();
    replacement.block_on(async {
        first
            .complete
            .committed()
            .await
            .expect("the ordinary waiter restarts and completes the first write");
        successor
            .complete
            .committed()
            .await
            .expect("the already queued successor completes without another enqueue");
        wait_for_lane_idle(&lane).await;
    });

    assert_eq!(store.matched_calls(), 1);
    let events = replacement
        .block_on(inner.events_since(&id(), 0))
        .expect("events remain readable");
    assert_eq!(events.len(), 2);
    assert!(format!("{:?}", events[0].kind).contains("runtime-drop-first-before-store"));
    assert!(format!("{:?}", events[1].kind).contains("runtime-drop-successor-before-store"));
}

#[test]
fn an_ordinary_write_replays_the_same_identity_after_runtime_stops_inside_store() {
    let origin = current_thread_runtime();
    let inner = Arc::new(FakeStore::default());
    let seeded = record();
    origin.block_on(inner.seed(&seeded)).expect("seed record");
    let store = Arc::new(CommitFaultStore::new(
        Arc::clone(&inner),
        is_runtime_drop_probe,
    ));
    store.pause_next();
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let lane = service.event_logs.lane(&id());

    let (first, successor) = origin.block_on(async {
        let first = lane.enqueue_commit(
            super::CommitMode::UpdateExact,
            seeded.clone(),
            vec![said(0, "runtime-drop-first-inside-store")],
        );
        let successor = lane.enqueue_commit(
            super::CommitMode::UpdateExact,
            seeded,
            vec![said(0, "runtime-drop-successor-inside-store")],
        );
        store.wait_until_held().await;
        (first, successor)
    });
    let first_write_id = store
        .matched_write_ids()
        .into_iter()
        .next()
        .expect("the paused call records its write identity");

    drop(origin);
    assert!(
        !lane
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .running,
        "the in-store cancellation releases the lane"
    );

    let replacement = current_thread_runtime();
    replacement.block_on(async {
        first
            .complete
            .committed()
            .await
            .expect("the first write replays on the replacement runtime");
        successor
            .complete
            .committed()
            .await
            .expect("the queued successor keeps FIFO order");
        wait_for_lane_idle(&lane).await;
    });

    let write_ids = store.matched_write_ids();
    assert_eq!(write_ids, [first_write_id.clone(), first_write_id]);
    let events = replacement.block_on(inner.events_since(&id(), 0)).unwrap();
    assert_eq!(events.len(), 2);
    assert!(format!("{:?}", events[0].kind).contains("runtime-drop-first-inside-store"));
    assert!(format!("{:?}", events[1].kind).contains("runtime-drop-successor-inside-store"));
}

#[test]
fn an_unknown_commit_outcome_replays_once_with_the_same_identity_on_a_new_runtime() {
    let origin = current_thread_runtime();
    let inner = Arc::new(FakeStore::default());
    let seeded = record();
    origin.block_on(inner.seed(&seeded)).expect("seed record");
    let store = Arc::new(CommitFaultStore::new(
        Arc::clone(&inner),
        is_runtime_drop_probe,
    ));
    store.pause_after_next();
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let lane = service.event_logs.lane(&id());

    let (first, successor) = origin.block_on(async {
        let first = lane.enqueue_commit(
            super::CommitMode::UpdateExact,
            seeded.clone(),
            vec![said(0, "runtime-drop-first-after-commit")],
        );
        let successor = lane.enqueue_commit(
            super::CommitMode::UpdateExact,
            seeded,
            vec![said(0, "runtime-drop-successor-after-commit")],
        );
        store.wait_until_held().await;
        assert_eq!(inner.events_since(&id(), 0).await.unwrap().len(), 1);
        (first, successor)
    });
    let first_write_id = store.matched_write_ids()[0].clone();

    drop(origin);
    assert!(
        !lane
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .running,
        "the post-commit cancellation releases the lane"
    );

    let replacement = current_thread_runtime();
    replacement.block_on(async {
        first
            .complete
            .committed()
            .await
            .expect("the durable receipt reconciles the unknown outcome");
        successor
            .complete
            .committed()
            .await
            .expect("the queued successor follows the reconciled write");
        wait_for_lane_idle(&lane).await;
    });

    let write_ids = store.matched_write_ids();
    assert_eq!(write_ids, [first_write_id.clone(), first_write_id]);
    let events = replacement.block_on(inner.events_since(&id(), 0)).unwrap();
    assert_eq!(
        events.len(),
        2,
        "the durable first event is never duplicated"
    );
    assert!(format!("{:?}", events[0].kind).contains("runtime-drop-first-after-commit"));
    assert!(format!("{:?}", events[1].kind).contains("runtime-drop-successor-after-commit"));
}

#[test]
fn fourth_outage_observation_and_fifo_progress_survive_worker_runtime_replacement() {
    let origin = current_thread_runtime();
    let store = Arc::new(FaultStore::default());
    let seeded = record();
    origin
        .block_on(store.inner.seed(&seeded))
        .expect("seed record");
    store.failures.store(4, Ordering::SeqCst);
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let lane = service.event_logs.lane(&id());

    let (first, successor) = origin.block_on(async {
        let first = lane.enqueue_commit(
            super::CommitMode::UpdateExact,
            seeded.clone(),
            vec![said(0, "runtime-drop-first-fourth-outage")],
        );
        let successor = lane.enqueue_commit(
            super::CommitMode::UpdateExact,
            seeded,
            vec![said(0, "runtime-drop-successor-fourth-outage")],
        );
        tokio::time::timeout(Duration::from_secs(2), lane.failure_delivered.notified())
            .await
            .expect("the fourth outage is delivered before observation");
        (first, successor)
    });
    assert_eq!(store.calls.load(Ordering::SeqCst), 4);

    drop(origin);
    assert!(
        !lane
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .running,
        "the pending acknowledgement is restartable"
    );

    let replacement = current_thread_runtime();
    replacement.block_on(async {
        assert!(matches!(
            first
                .complete
                .committed()
                .await
                .expect_err("the replacement runtime observes the retained report"),
            StoreError::Unavailable(_)
        ));
        successor
            .complete
            .committed()
            .await
            .expect("the rejected reservation is removed before its successor runs");
        wait_for_lane_idle(&lane).await;
    });

    assert_eq!(store.calls.load(Ordering::SeqCst), 5);
    let events = replacement
        .block_on(store.inner.events_since(&id(), 0))
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].seq, 1);
    assert!(format!("{:?}", events[0].kind).contains("runtime-drop-successor-fourth-outage"));
}

#[tokio::test]
async fn a_retained_continuation_survives_fourth_error_delivery_without_polling() {
    let store = Arc::new(FaultStore::default());
    let seeded = record();
    store.inner.seed(&seeded).await.expect("seed record");
    store.failures.store(4, Ordering::SeqCst);
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let lane = service.event_logs.lane(&id());
    let continued = Arc::new(AtomicBool::new(false));
    let continuation_flag = Arc::clone(&continued);
    let pending = service.persist_durably_with(
        seeded,
        vec![said(0, "retained-after-unpolled-report")],
        move |_committed| async move {
            continuation_flag.store(true, Ordering::SeqCst);
            ((), ())
        },
        super::ignore_rejected,
        super::drop_undelivered,
    );

    tokio::time::timeout(Duration::from_secs(1), lane.failure_delivered.notified())
        .await
        .expect("the fourth outage response is delivered");
    drop(pending);

    let event = wait_for_event(&store.inner).await;
    assert!(format!("{:?}", event.kind).contains("retained-after-unpolled-report"));
    tokio::time::timeout(Duration::from_secs(1), async {
        while !continued.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("eventual success runs the retained continuation");
    assert_eq!(store.calls.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn an_error_delivered_to_the_waiter_ends_the_operation_permanently() {
    let store = Arc::new(FaultStore::default());
    let mut update = record();
    store.inner.seed(&update).await.expect("seed record");
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
    store.inner.seed(&base).await.expect("seed record");
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
    store.inner.seed(&seeded).await.expect("seed record");
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
        .seed(&durable)
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
    store.seed(&duplicate).await.expect("seed existing record");
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
    store.inner.seed(&update).await.expect("seed record");
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
    store.inner.seed(&base).await.expect("seed record");
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
async fn terminal_detach_rebases_after_a_competing_termination() {
    let store = Arc::new(FaultStore::default());
    let base = record();
    store.inner.seed(&base).await.expect("seed record");
    let mut attached = base;
    attached.session.last_seq = 1;
    store
        .inner
        .commit_session_idempotent(
            "terminal-attached",
            &attached,
            &[Event {
                session_id: id(),
                seq: 1,
                kind: EventKind::TerminalAttached,
                appended_at: String::new(),
            }],
        )
        .await
        .expect("seed a durable attachment");
    let attached = store
        .inner
        .get(&id())
        .await
        .unwrap()
        .expect("attached record exists");
    store.pause_next_write.store(true, Ordering::SeqCst);
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));

    let task_service = Arc::clone(&service);
    let detach = tokio::spawn(async move {
        let mut detached = attached;
        let result = task_service
            .persist_durably(
                &mut detached,
                vec![Event {
                    session_id: id(),
                    seq: 0,
                    kind: EventKind::TerminalDetached,
                    appended_at: String::new(),
                }],
            )
            .await;
        (result, detached)
    });
    store.write_started.notified().await;

    let mut terminated = store
        .inner
        .get(&id())
        .await
        .unwrap()
        .expect("the competing replica reads the attachment");
    terminated.session.status = SessionStatus::Terminated;
    terminated.session.stop_reason = Some(StopReason::UserTerminated);
    terminated.session.last_seq = 2;
    store
        .inner
        .commit_session_idempotent(
            "competing-termination",
            &terminated,
            &[Event {
                session_id: id(),
                seq: 2,
                kind: EventKind::StatusChanged {
                    status: SessionStatus::Terminated,
                    stop_reason: Some(StopReason::UserTerminated),
                    detail: None,
                    capability: None,
                },
                appended_at: String::new(),
            }],
        )
        .await
        .expect("the competing termination commits first");
    store.release_write.notify_one();

    let (result, detached) = detach.await.expect("detach task completes");
    result.expect("cleanup-only detach rebases onto terminal state");
    assert_eq!(detached.session.status, SessionStatus::Terminated);
    assert_eq!(detached.session.last_seq, 3);

    let durable = store.inner.get(&id()).await.unwrap().unwrap();
    assert_eq!(durable.session.status, SessionStatus::Terminated);
    assert_eq!(durable.session.last_seq, 3);
    assert_eq!(durable.revision, 3);
    let events = store.inner.events_since(&id(), 0).await.unwrap();
    assert_eq!(events.len(), 3);
    assert!(matches!(
        events[0].kind,
        crate::event::StoredEventKind::Known(EventKind::TerminalAttached)
    ));
    assert!(matches!(
        events[1].kind,
        crate::event::StoredEventKind::Known(EventKind::StatusChanged {
            status: SessionStatus::Terminated,
            ..
        })
    ));
    assert!(matches!(
        events[2].kind,
        crate::event::StoredEventKind::Known(EventKind::TerminalDetached)
    ));
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
    let (input, mut runtime_input) = terminal_input_channel(1);
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

    let retry_geometry = TerminalGeometry::new(120, 42).expect("valid retry geometry");
    service
        .resize_terminal(&owner, &created.id, retry_geometry)
        .await
        .expect("a later resize is compared with durable state, not the rejected proposal");
    wait_for_lane_idle(&lane).await;

    let durable = store.inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.session.geometry, retry_geometry);
    assert_eq!(durable.session.last_seq, before_tail + 1);
    assert_eq!(
        service
            .entries
            .read()
            .await
            .get(&created.id)
            .expect("live entry")
            .session
            .geometry,
        retry_geometry
    );
    assert_eq!(
        service
            .screen_snapshot(&owner, &created.id)
            .await
            .expect("screen remains readable")
            .geometry,
        retry_geometry
    );
    assert!(matches!(
        runtime_input.try_recv(),
        Ok(layer3_adapter::TerminalInput::Resize(_))
    ));
}

#[tokio::test]
async fn cancelling_a_resize_before_delivery_keeps_desired_geometry_for_retry() {
    let store = Arc::new(FaultStore::default());
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let owner = user("cancelled-resize-owner");
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
    let before = store.inner.get(&created.id).await.unwrap().unwrap();
    let before_screen = service
        .screen_snapshot(&owner, &created.id)
        .await
        .expect("screen is readable");
    let (input, mut runtime_input) = terminal_input_channel(4);
    service
        .entries
        .write()
        .await
        .get_mut(&created.id)
        .expect("live entry")
        .input = Some(input);
    let resized = TerminalGeometry::new(121, 43).expect("valid geometry");

    store.pause_next_write.store(true, Ordering::SeqCst);
    let resize = tokio::spawn({
        let service = Arc::clone(&service);
        let owner = owner.clone();
        let id = created.id.clone();
        async move { service.resize_terminal(&owner, &id, resized).await }
    });
    store.write_started.notified().await;
    resize.abort();
    let _ = resize.await;
    store.release_write.notify_one();
    drop(
        tokio::time::timeout(Duration::from_secs(2), service.lock_lifecycle(&created.id))
            .await
            .expect("a public lock restarts and drains the retained resize"),
    );
    wait_for_lane_idle(&lane).await;

    let durable = store.inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.session.geometry, resized);
    assert_eq!(durable.session.last_seq, before.session.last_seq + 1);
    assert_eq!(
        service
            .entries
            .read()
            .await
            .get(&created.id)
            .expect("the live entry remains")
            .session
            .geometry,
        before.session.geometry,
        "caller cancellation before delivery leaves live geometry unchanged"
    );
    assert_eq!(
        service
            .screen_snapshot(&owner, &created.id)
            .await
            .expect("screen is readable")
            .geometry,
        before_screen.geometry,
        "caller cancellation before delivery leaves the screen unchanged"
    );
    assert!(matches!(
        runtime_input.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));

    let later = TerminalGeometry::new(122, 44).expect("valid later geometry");
    service
        .resize_terminal(&owner, &created.id, later)
        .await
        .expect("a later resize follows the retained one");
    wait_for_lane_idle(&lane).await;
    let durable = store.inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.session.geometry, later);
    assert_eq!(durable.session.last_seq, before.session.last_seq + 2);
    assert!(matches!(
        runtime_input.recv().await,
        Some(layer3_adapter::TerminalInput::Resize(size))
            if size.cols == later.columns() && size.rows == later.rows()
    ));
}

#[tokio::test]
async fn four_observed_store_outages_leave_a_resize_entirely_unpublished() {
    let store = Arc::new(FaultStore::default());
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let owner = user("resize-outage-owner");
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
    let before = store.inner.get(&created.id).await.unwrap().unwrap();
    let before_screen = service
        .screen_snapshot(&owner, &created.id)
        .await
        .expect("screen is readable");
    let (input, mut runtime_input) = terminal_input_channel(1);
    service
        .entries
        .write()
        .await
        .get_mut(&created.id)
        .expect("live entry")
        .input = Some(input);
    store.calls.store(0, Ordering::SeqCst);
    store.failures.store(4, Ordering::SeqCst);

    let error = service
        .resize_terminal(
            &owner,
            &created.id,
            TerminalGeometry::new(123, 45).expect("valid geometry"),
        )
        .await
        .expect_err("the fourth observed outage is reported");
    assert!(matches!(error, crate::CoreError::RuntimeUnavailable(_)));
    let gate = service.lifecycle_gates.gate(&created.id);
    drop(gate.lock().await);
    wait_for_lane_idle(&lane).await;
    assert_eq!(store.calls.load(Ordering::SeqCst), 4);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(store.calls.load(Ordering::SeqCst), 4);

    let durable = store.inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.session.geometry, before.session.geometry);
    assert_eq!(durable.session.last_seq, before.session.last_seq);
    let entry_geometry = service
        .entries
        .read()
        .await
        .get(&created.id)
        .expect("live entry")
        .session
        .geometry;
    assert_eq!(entry_geometry, before.session.geometry);
    assert_eq!(
        service
            .screen_snapshot(&owner, &created.id)
            .await
            .expect("screen remains readable")
            .geometry,
        before_screen.geometry
    );
    assert!(matches!(
        runtime_input.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    assert!(store
        .inner
        .events_since(&created.id, before.session.last_seq)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_resize_success_does_not_regress_a_queued_successor_cursor() {
    let store = Arc::new(FaultStore::default());
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let owner = user("resize-successor-owner");
    let created = service
        .create_session(&owner, spec())
        .await
        .expect("session is created");
    await_settled(&service, &owner, &created.id).await;
    let lane = service.event_logs.lane(&created.id);
    wait_for_lane_idle(&lane).await;
    let before = store.inner.get(&created.id).await.unwrap().unwrap();
    let resized = TerminalGeometry::new(124, 46).expect("valid geometry");

    store.pause_next_write.store(true, Ordering::SeqCst);
    let resize = tokio::spawn({
        let service = Arc::clone(&service);
        let owner = owner.clone();
        let id = created.id.clone();
        async move { service.resize_terminal(&owner, &id, resized).await }
    });
    store.write_started.notified().await;
    {
        let mut entries = service.entries.write().await;
        let entry = entries.get_mut(&created.id).expect("live entry");
        super::append(
            entry,
            EventKind::MessageCommitted {
                content: "queued-after-resize".into(),
            },
        );
    }
    store.release_write.notify_one();
    resize
        .await
        .expect("resize task completes")
        .expect("resize commits");
    wait_for_lane_idle(&lane).await;

    let durable = store.inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.session.geometry, resized);
    assert_eq!(durable.session.last_seq, before.session.last_seq + 2);
    assert_eq!(
        service
            .entries
            .read()
            .await
            .get(&created.id)
            .expect("live entry")
            .session
            .last_seq,
        durable.session.last_seq
    );
}

#[tokio::test]
async fn a_rejected_resize_rebuilds_a_queued_successor_cursor() {
    let store = Arc::new(FaultStore::default());
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let owner = user("resize-rejected-successor-owner");
    let created = service
        .create_session(&owner, spec())
        .await
        .expect("session is created");
    await_settled(&service, &owner, &created.id).await;
    let lane = service.event_logs.lane(&created.id);
    wait_for_lane_idle(&lane).await;
    let before = store.inner.get(&created.id).await.unwrap().unwrap();
    let local = TerminalGeometry::new(125, 47).expect("valid geometry");
    let rival_geometry = TerminalGeometry::new(126, 48).expect("valid geometry");

    store.pause_next_write.store(true, Ordering::SeqCst);
    let resize = tokio::spawn({
        let service = Arc::clone(&service);
        let owner = owner.clone();
        let id = created.id.clone();
        async move { service.resize_terminal(&owner, &id, local).await }
    });
    store.write_started.notified().await;
    {
        let mut entries = service.entries.write().await;
        let entry = entries.get_mut(&created.id).expect("live entry");
        super::append(
            entry,
            EventKind::MessageCommitted {
                content: "survives-rejected-resize".into(),
            },
        );
    }
    let mut rival = store.inner.get(&created.id).await.unwrap().unwrap();
    rival.session.geometry = rival_geometry;
    store
        .inner
        .commit_session_idempotent("rival-rejects-resize-with-successor", &rival, &[])
        .await
        .expect("rival geometry commits");
    store.release_write.notify_one();
    assert!(resize.await.expect("resize task completes").is_err());
    let gate = service.lifecycle_gates.gate(&created.id);
    drop(gate.lock().await);
    wait_for_lane_idle(&lane).await;

    let durable = store.inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.session.geometry, rival_geometry);
    assert_eq!(durable.session.last_seq, before.session.last_seq + 1);
    let events = store
        .inner
        .events_since(&created.id, before.session.last_seq)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert!(format!("{:?}", events[0].kind).contains("survives-rejected-resize"));
    assert_eq!(
        service
            .entries
            .read()
            .await
            .get(&created.id)
            .expect("live entry")
            .session
            .last_seq,
        durable.session.last_seq
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn final_rejection_insertion_is_atomic_against_a_late_producer() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(
        Arc::clone(&inner),
        is_final_rejection_probe,
    ));
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let owner = user("atomic-rejection-owner");
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
    let before = inner.get(&created.id).await.unwrap().unwrap();
    let rejected_geometry = TerminalGeometry::new(127, 49).expect("valid geometry");
    let rival_geometry = TerminalGeometry::new(128, 50).expect("valid geometry");
    let lifecycle_gate = service.lifecycle_gates.gate(&created.id);
    let (rejection_reached, release_rejection) = lane.pause_next_final_rejection();

    store.pause_next();
    let resize = tokio::spawn({
        let service = Arc::clone(&service);
        let owner = owner.clone();
        let id = created.id.clone();
        async move {
            service
                .resize_terminal(&owner, &id, rejected_geometry)
                .await
        }
    });
    store.wait_until_held().await;
    {
        let mut entries = service.entries.write().await;
        let entry = entries.get_mut(&created.id).expect("live entry");
        super::append(
            entry,
            EventKind::MessageCommitted {
                content: "early-before-final-rejection".into(),
            },
        );
    }
    let mut rival = before.clone();
    rival.session.geometry = rival_geometry;
    inner
        .commit_session_idempotent("rival-at-final-rejection", &rival, &[])
        .await
        .expect("the rival geometry commits");
    store.release();

    tokio::task::spawn_blocking(move || {
        rejection_reached
            .recv_timeout(Duration::from_secs(2))
            .expect("the rejection reaches its locked insertion boundary");
    })
    .await
    .expect("the insertion observer joins");
    assert!(
        lane.state.try_lock().is_err(),
        "the finalizer holds the queue lock at the insertion boundary"
    );

    // The next selected store call is the late producer. Holding it lets the
    // test inspect whether retained rejection cleanup ran before that producer.
    store.pause_next();
    let (late_started, late_reached) = tokio::sync::oneshot::channel();
    let late = tokio::spawn({
        let service = Arc::clone(&service);
        let id = created.id.clone();
        async move {
            let mut entries = service.entries.write().await;
            let entry = entries.get_mut(&id).expect("live entry");
            let _ = late_started.send(());
            super::append(
                entry,
                EventKind::MessageCommitted {
                    content: "late-after-final-rejection".into(),
                },
            );
        }
    });
    late_reached
        .await
        .expect("the late producer reaches the lane insertion boundary");
    assert!(
        !late.is_finished(),
        "the final rejection still owns the lane lock at the linearization point"
    );
    release_rejection
        .send(())
        .expect("release the final rejection transition");
    late.await.expect("the late producer enqueues");

    store.wait_until_held().await;
    drop(
        lifecycle_gate
            .try_lock()
            .expect("retained rejection cleanup precedes the late store call"),
    );
    assert!(
        resize.await.expect("the resize task completes").is_err(),
        "the conflicting resize remains rejected"
    );
    store.release();
    wait_for_lane_idle(&lane).await;

    let durable = inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.session.geometry, rival_geometry);
    assert_eq!(durable.session.last_seq, before.session.last_seq + 2);
    let events = inner
        .events_since(&created.id, before.session.last_seq)
        .await
        .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].seq, before.session.last_seq + 1);
    assert!(format!("{:?}", events[0].kind).contains("early-before-final-rejection"));
    assert_eq!(events[1].seq, before.session.last_seq + 2);
    assert!(format!("{:?}", events[1].kind).contains("late-after-final-rejection"));
    assert_eq!(
        service
            .entries
            .read()
            .await
            .get(&created.id)
            .expect("live entry")
            .session
            .last_seq,
        durable.session.last_seq
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
    store.inner.seed(&base).await.expect("seed record");
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
async fn an_empty_barrier_cannot_replay_a_rejected_status_snapshot() {
    let store = Arc::new(FaultStore::default());
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let owner = user("status-barrier-owner");
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

    store.pause_next_write.store(true, Ordering::SeqCst);
    {
        let mut entries = service.entries.write().await;
        let entry = entries.get_mut(&created.id).expect("live entry");
        entry.session.status = SessionStatus::Running;
        super::append(
            entry,
            EventKind::StatusChanged {
                status: SessionStatus::Running,
                stop_reason: None,
                detail: None,
                capability: None,
            },
        );
    }
    store.write_started.notified().await;

    let flush_service = Arc::clone(&service);
    let flush_id = created.id.clone();
    let barrier = tokio::spawn(async move { flush_service.flush(&flush_id).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while lane.queued() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the empty barrier queues behind the held status event");

    let mut rival = before.clone();
    rival.session.status = SessionStatus::Idle;
    rival.session.stop_reason = Some(StopReason::UserTerminated);
    rival.session.recovery_state = RecoveryState::Idle;
    store
        .inner
        .commit_session_idempotent("rival-status-before-barrier", &rival, &[])
        .await
        .expect("the rival lifecycle state commits first");
    store.release_write.notify_one();

    barrier.await.expect("the empty barrier finishes");
    wait_for_lane_idle(&lane).await;
    let durable = store.inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.session.status, SessionStatus::Idle);
    assert_eq!(durable.session.recovery_state, RecoveryState::Idle);
    assert_eq!(
        durable.session.stop_reason,
        Some(StopReason::UserTerminated)
    );
    assert_eq!(durable.session.last_seq, before.session.last_seq);
    assert!(store
        .inner
        .events_since(&created.id, before.session.last_seq)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_post_rejection_flush_owns_no_stale_semantic_fields() {
    let store = Arc::new(FaultStore::default());
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let owner = user("post-rejection-flush-owner");
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
    assert!(
        before.runtime_id.is_some(),
        "the settled session has a runtime"
    );

    store.pause_next_write.store(true, Ordering::SeqCst);
    {
        let mut entries = service.entries.write().await;
        let entry = entries.get_mut(&created.id).expect("live entry");
        entry.session.owner = SubjectId("stale-owner".into());
        entry.session.kernel = KernelId::new("stale-kernel");
        entry.session.status = SessionStatus::Running;
        entry.session.stop_reason = None;
        entry.session.geometry = TerminalGeometry::new(117, 39).expect("valid geometry");
        entry.session.options.mode = SessionMode::Conversation;
        entry.runtime = None;
        entry.context_ref = Some("stale-context".into());
        super::append(
            entry,
            EventKind::StatusChanged {
                status: SessionStatus::Running,
                stop_reason: None,
                detail: None,
                capability: None,
            },
        );
    }
    store.write_started.notified().await;

    let mut rival = before.clone();
    rival.session.status = SessionStatus::Idle;
    rival.session.stop_reason = Some(StopReason::UserTerminated);
    rival.session.recovery_state = RecoveryState::Idle;
    store
        .inner
        .commit_session_idempotent("rival-before-post-rejection-flush", &rival, &[])
        .await
        .expect("the rival lifecycle state commits first");
    store.release_write.notify_one();
    wait_for_lane_idle(&lane).await;

    // This barrier is deliberately enqueued only after the event-bearing commit
    // has been rejected and its dropped receiver has allowed the lane to settle.
    service.flush(&created.id).await;
    wait_for_lane_idle(&lane).await;

    let durable = store.inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.session.owner, rival.session.owner);
    assert_eq!(durable.session.kernel, rival.session.kernel);
    assert_eq!(durable.session.status, rival.session.status);
    assert_eq!(durable.session.recovery_state, rival.session.recovery_state);
    assert_eq!(durable.session.stop_reason, rival.session.stop_reason);
    assert_eq!(durable.session.geometry, rival.session.geometry);
    assert_eq!(durable.session.options, rival.session.options);
    assert_eq!(durable.runtime_id, rival.runtime_id);
    assert_eq!(durable.context_ref, rival.context_ref);
    assert_eq!(durable.session.last_seq, before.session.last_seq);
    assert!(store
        .inner
        .events_since(&created.id, before.session.last_seq)
        .await
        .unwrap()
        .is_empty());
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
async fn fresh_lane_rotation_finalizers_refuse_a_successor_claim() {
    #[derive(Clone, Copy)]
    enum Finalizer {
        Complete,
        ClearClaim,
        RollBack,
    }

    for (case, finalizer) in [
        ("completion", Finalizer::Complete),
        ("claim clear", Finalizer::ClearClaim),
        ("rollback", Finalizer::RollBack),
    ] {
        let store = Arc::new(FakeStore::default());
        let mut claimed = record();
        claimed.session.status = match finalizer {
            Finalizer::ClearClaim => SessionStatus::Ready,
            Finalizer::Complete | Finalizer::RollBack => SessionStatus::Rotating,
        };
        claimed.runtime_id = Some("runtime-observed".into());
        claimed.rotation_claim = Some("rotation-observed".into());
        store.seed(&claimed).await.expect("seed claimed handover");

        let mut successor = claimed.clone();
        successor.rotation_claim = Some("rotation-successor".into());
        successor.context_ref = Some(format!("successor-context-{case}"));
        store
            .commit_session_idempotent(
                &format!("rotation-finalizer-successor-{case}"),
                &successor,
                &[],
            )
            .await
            .unwrap_or_else(|error| panic!("{case} successor commits: {error}"));

        // Construct the service only after the successor commits. Its lane has
        // no predecessor until the finalizer seeds the stale claimed boundary.
        let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
        let mut intended = claimed.clone();
        let result = match finalizer {
            Finalizer::Complete => {
                intended.session.status = SessionStatus::Ready;
                service
                    .complete_handover_durably(
                        &mut intended,
                        &claimed,
                        vec![Event {
                            session_id: id(),
                            seq: 0,
                            kind: EventKind::StatusChanged {
                                status: SessionStatus::Ready,
                                stop_reason: None,
                                detail: None,
                                capability: None,
                            },
                            appended_at: String::new(),
                        }],
                    )
                    .await
            }
            Finalizer::ClearClaim => {
                intended.rotation_claim = None;
                service
                    .clear_rotation_claim_durably(&mut intended, &claimed)
                    .await
            }
            Finalizer::RollBack => {
                intended.session.status = SessionStatus::Ready;
                intended.rotation_claim = None;
                service
                    .roll_back_handover_durably(
                        &mut intended,
                        &claimed,
                        vec![Event {
                            session_id: id(),
                            seq: 0,
                            kind: EventKind::StatusChanged {
                                status: SessionStatus::Ready,
                                stop_reason: None,
                                detail: Some("rollback".into()),
                                capability: None,
                            },
                            appended_at: String::new(),
                        }],
                    )
                    .await
            }
        };
        let error = result.expect_err("the stale finalizer cannot inherit a successor claim");
        assert!(
            matches!(error, StoreError::RevisionConflict { .. }),
            "{case} returned {error:?}"
        );

        let durable = store.get(&id()).await.unwrap().unwrap();
        assert_eq!(durable.revision, successor.revision + 1, "{case} revision");
        assert_eq!(
            durable.session.status, successor.session.status,
            "{case} status"
        );
        assert_eq!(
            durable.rotation_claim, successor.rotation_claim,
            "{case} claim"
        );
        assert_eq!(durable.runtime_id, successor.runtime_id, "{case} runtime");
        assert_eq!(durable.context_ref, successor.context_ref, "{case} context");
        assert!(
            store.events_since(&id(), 0).await.unwrap().is_empty(),
            "{case} appends no stale event"
        );
    }
}

#[tokio::test]
async fn claim_minting_ready_publication_refuses_a_successor_runtime() {
    let store = Arc::new(FakeStore::default());
    store.disable_coordination();
    let old_grant = Some(vec![crate::environment::GrantedSkill {
        id: crate::SkillId::new("old-runtime-skill"),
        version: Some("1".into()),
    }]);
    let mut claimed = record();
    claimed.session.status = SessionStatus::Rotating;
    claimed.runtime_id = Some("runtime-readopted".into());
    claimed.session.granted_skills.clone_from(&old_grant);
    store
        .seed(&claimed)
        .await
        .expect("seed claimless readoption");

    let mut successor = claimed.clone();
    successor.runtime_id = Some("runtime-successor".into());
    successor.context_ref = Some("successor-context".into());
    store
        .commit_session_idempotent("claimless-ready-successor", &successor, &[])
        .await
        .expect("the compatibility successor switches runtime first");

    // Constructing the service after the switch gives its lane no local
    // predecessor. Publication must retain the stale claimed boundary as its
    // compare predicate rather than treating the newer revision as authority.
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let candidate_packs = vec![crate::store::SkillPack {
        name: "successor-skill".into(),
        version: Some("2".into()),
        files: vec![("SKILL.md".into(), b"successor bytes".to_vec())],
    }];
    let candidate_grant = Some(vec![crate::environment::GrantedSkill {
        id: crate::SkillId::new("successor-skill"),
        version: Some("2".into()),
    }]);
    let mut intended = claimed.clone();
    intended.session.status = SessionStatus::Ready;
    intended.rotation_claim = Some("rotation-minted-by-readoption".into());
    intended.session.granted_skills = candidate_grant;
    intended.skill_packs = crate::store::SkillPackState::Staged(candidate_packs);

    let error = service
        .publish_ready_durably(
            &mut intended,
            &claimed,
            vec![Event {
                session_id: id(),
                seq: 0,
                kind: EventKind::StatusChanged {
                    status: SessionStatus::Ready,
                    stop_reason: None,
                    detail: None,
                    capability: None,
                },
                appended_at: String::new(),
            }],
        )
        .await
        .expect_err("stale Ready publication cannot inherit successor authority");
    assert!(matches!(error, StoreError::RevisionConflict { .. }));

    let durable = store.get(&id()).await.unwrap().unwrap();
    assert_eq!(durable.session.status, SessionStatus::Rotating);
    assert_eq!(durable.runtime_id, successor.runtime_id);
    assert_eq!(durable.rotation_claim, None);
    assert_eq!(durable.session.granted_skills, old_grant);
    assert_eq!(durable.skill_packs, crate::store::SkillPackState::Legacy);
    assert_eq!(durable.context_ref, successor.context_ref);
    assert!(store.events_since(&id(), 0).await.unwrap().is_empty());
}

#[tokio::test]
async fn fresh_lane_compatibility_handover_inherits_unrelated_history() {
    let store = Arc::new(FakeStore::default());
    store.disable_coordination();
    let mut claimed = record();
    claimed.session.status = SessionStatus::Rotating;
    claimed.session.options.mode = SessionMode::Terminal;
    claimed.runtime_id = None;
    claimed.rotation_claim = Some("rotation-cleared-h1".into());
    claimed.pending_handover = Some(pending_plane_handover("rotation-cleared-h1"));
    store.seed(&claimed).await.expect("seed cleared handover");

    let mut rival = claimed.clone();
    rival.context_ref = Some("rival-after-h1-clear".into());
    rival.session.last_seq = 1;
    store
        .commit_session_idempotent(
            "rival-after-h1-clear",
            &rival,
            &[said(1, "history-after-h1-clear")],
        )
        .await
        .expect("rival history commits");

    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let mut intended = claimed.clone();
    intended.session.options.mode = SessionMode::Conversation;
    intended.session.rotations = 1;
    service
        .complete_compatibility_handover_durably(
            &mut intended,
            &claimed,
            vec![Event {
                session_id: id(),
                seq: 0,
                kind: EventKind::PlaneChanged {
                    from: SessionMode::Terminal,
                    to: SessionMode::Conversation,
                    snapshot_at: "2026-08-24T01:00:00Z".into(),
                },
                appended_at: String::new(),
            }],
        )
        .await
        .expect("the target semantics rebase over unrelated history");

    assert_eq!(intended.session.status, SessionStatus::Rotating);
    assert_eq!(intended.session.options.mode, SessionMode::Conversation);
    assert_eq!(intended.session.rotations, 1);
    assert_eq!(intended.runtime_id, None);
    assert_eq!(
        intended.rotation_claim.as_deref(),
        Some("rotation-cleared-h1")
    );
    assert_eq!(intended.pending_handover, claimed.pending_handover);
    assert_eq!(
        intended.context_ref.as_deref(),
        Some("rival-after-h1-clear")
    );
    assert_eq!(intended.session.last_seq, 2);

    let events = store.events_since(&id(), 0).await.unwrap();
    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        [1, 2]
    );
    assert!(format!("{:?}", events[0].kind).contains("history-after-h1-clear"));
    assert!(matches!(
        events[1].kind,
        crate::event::StoredEventKind::Known(EventKind::PlaneChanged {
            from: SessionMode::Terminal,
            to: SessionMode::Conversation,
            ..
        })
    ));
}

#[tokio::test]
async fn fresh_lane_compatibility_handover_refuses_successor_authority() {
    enum SuccessorChange {
        Status,
        RotationClaim,
        Runtime,
        PendingHandover,
    }

    for (case, change) in [
        ("status", SuccessorChange::Status),
        ("rotation claim", SuccessorChange::RotationClaim),
        ("runtime", SuccessorChange::Runtime),
        ("pending handover", SuccessorChange::PendingHandover),
    ] {
        let store = Arc::new(FakeStore::default());
        store.disable_coordination();
        let mut claimed = record();
        claimed.session.status = SessionStatus::Rotating;
        claimed.session.options.mode = SessionMode::Terminal;
        claimed.runtime_id = None;
        claimed.rotation_claim = Some("rotation-cleared-observed".into());
        claimed.pending_handover = Some(pending_plane_handover("rotation-cleared-observed"));
        store.seed(&claimed).await.expect("seed cleared handover");

        let mut successor = claimed.clone();
        successor.context_ref = Some(format!("successor-context-{case}"));
        match change {
            SuccessorChange::Status => successor.session.status = SessionStatus::Ready,
            SuccessorChange::RotationClaim => {
                successor.rotation_claim = Some("rotation-cleared-successor".into())
            }
            SuccessorChange::Runtime => successor.runtime_id = Some("runtime-successor".into()),
            SuccessorChange::PendingHandover => {
                successor
                    .pending_handover
                    .as_mut()
                    .expect("the observed handover has a descriptor")
                    .snapshot_ref = format!("sessions/{}/successor.snapshot", id().0);
            }
        }
        store
            .commit_session_idempotent(
                &format!("cleared-handover-successor-{case}"),
                &successor,
                &[],
            )
            .await
            .unwrap_or_else(|error| panic!("{case} successor commits: {error}"));

        let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
        let mut intended = claimed.clone();
        intended.session.options.mode = SessionMode::Conversation;
        intended.session.rotations = 1;
        let error = service
            .complete_compatibility_handover_durably(
                &mut intended,
                &claimed,
                vec![Event {
                    session_id: id(),
                    seq: 0,
                    kind: EventKind::PlaneChanged {
                        from: SessionMode::Terminal,
                        to: SessionMode::Conversation,
                        snapshot_at: "2026-08-24T01:00:00Z".into(),
                    },
                    appended_at: String::new(),
                }],
            )
            .await
            .expect_err("successor authority rejects stale target publication");
        assert!(
            matches!(error, StoreError::RevisionConflict { .. }),
            "{case} successor returned {error:?}"
        );

        let durable = store.get(&id()).await.unwrap().unwrap();
        assert_eq!(
            durable.session.status, successor.session.status,
            "{case} status"
        );
        assert_eq!(
            durable.rotation_claim, successor.rotation_claim,
            "{case} claim"
        );
        assert_eq!(durable.runtime_id, successor.runtime_id, "{case} runtime");
        assert_eq!(
            durable.pending_handover, successor.pending_handover,
            "{case} descriptor"
        );
        assert_eq!(durable.context_ref, successor.context_ref, "{case} context");
        assert_eq!(
            durable.session.options.mode,
            SessionMode::Terminal,
            "{case} target semantics remain unpublished"
        );
        assert!(
            store.events_since(&id(), 0).await.unwrap().is_empty(),
            "{case} appends no target event"
        );
    }
}

#[tokio::test]
async fn compatibility_handover_observation_preserves_queued_history_order() {
    let store = Arc::new(FaultStore::default());
    store.inner.disable_coordination();
    let mut claimed = record();
    claimed.session.status = SessionStatus::Rotating;
    claimed.session.options.mode = SessionMode::Terminal;
    claimed.runtime_id = None;
    claimed.rotation_claim = Some("rotation-queued-history".into());
    claimed.pending_handover = Some(pending_plane_handover("rotation-queued-history"));
    store
        .inner
        .seed(&claimed)
        .await
        .expect("seed cleared handover");
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let lane = service.event_logs.lane(&id());
    lane.observe_record(&claimed);
    wait_for_lane_idle(&lane).await;

    store.pause_next_write.store(true, Ordering::SeqCst);
    let mut before_record = claimed.clone();
    before_record.context_ref = Some("context-from-earlier-history".into());
    let before = lane.enqueue_commit(
        super::CommitMode::UpdateInheriting,
        before_record,
        vec![said(0, "history-before-target-publication")],
    );
    store.write_started.notified().await;

    let mut intended = claimed.clone();
    intended.session.options.mode = SessionMode::Conversation;
    intended.session.rotations = 1;
    let expected_pending_handover = claimed.pending_handover.clone();
    let target_record = intended.clone();
    let target = tokio::spawn({
        let service = Arc::clone(&service);
        async move {
            service
                .complete_compatibility_handover_durably(
                    &mut intended,
                    &claimed,
                    vec![Event {
                        session_id: id(),
                        seq: 0,
                        kind: EventKind::PlaneChanged {
                            from: SessionMode::Terminal,
                            to: SessionMode::Conversation,
                            snapshot_at: "2026-08-24T01:00:00Z".into(),
                        },
                        appended_at: String::new(),
                    }],
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while lane.queued() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the observation and target publication queue behind earlier history");
    let after = lane.enqueue_commit(
        super::CommitMode::UpdateInheriting,
        target_record,
        vec![said(0, "history-after-target-publication")],
    );

    store.release_write.notify_one();
    before
        .complete
        .committed()
        .await
        .expect("earlier history commits");
    target
        .await
        .expect("target publication task completes")
        .expect("target semantics commit");
    after
        .complete
        .committed()
        .await
        .expect("later history commits");
    wait_for_lane_idle(&lane).await;

    let durable = store.inner.get(&id()).await.unwrap().unwrap();
    assert_eq!(durable.session.options.mode, SessionMode::Conversation);
    assert_eq!(durable.session.rotations, 1);
    assert_eq!(durable.runtime_id, None);
    assert_eq!(
        durable.rotation_claim.as_deref(),
        Some("rotation-queued-history")
    );
    assert_eq!(durable.pending_handover, expected_pending_handover);
    assert_eq!(
        durable.context_ref.as_deref(),
        Some("context-from-earlier-history")
    );
    assert_eq!(durable.session.last_seq, 3);
    let events = store.inner.events_since(&id(), 0).await.unwrap();
    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert!(format!("{:?}", events[0].kind).contains("history-before-target-publication"));
    assert!(matches!(
        events[1].kind,
        crate::event::StoredEventKind::Known(EventKind::PlaneChanged { .. })
    ));
    assert!(format!("{:?}", events[2].kind).contains("history-after-target-publication"));
}

#[tokio::test]
async fn compatibility_runtime_clear_inherits_unrelated_context_and_history() {
    let store = Arc::new(FakeStore::default());
    store.disable_coordination();
    let mut claimed = record();
    claimed.session.status = SessionStatus::Idle;
    claimed.session.recovery_state = RecoveryState::Idle;
    claimed.runtime_id = Some("runtime-compatibility".into());
    claimed.rotation_claim = Some("rotation-stable".into());
    store.seed(&claimed).await.expect("seed claimed runtime");

    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let mut rival = claimed.clone();
    rival.context_ref = Some("rival-context".into());
    rival.session.last_seq = 1;
    store
        .commit_session_idempotent(
            "compatibility-runtime-rival",
            &rival,
            &[said(1, "rival-history")],
        )
        .await
        .expect("rival context and history commit");

    let mut intended = claimed.clone();
    intended.runtime_id = None;
    service
        .clear_compatibility_runtime_durably(&mut intended, &claimed)
        .await
        .expect("the runtime clear inherits unrelated durable work");

    assert_eq!(intended.runtime_id, None);
    assert_eq!(intended.context_ref.as_deref(), Some("rival-context"));
    assert_eq!(intended.session.last_seq, 1);
    assert_eq!(intended.session.status, SessionStatus::Idle);
    assert_eq!(intended.rotation_claim.as_deref(), Some("rotation-stable"));
    let events = store.events_since(&id(), 0).await.unwrap();
    assert_eq!(events.len(), 1);
    assert!(format!("{:?}", events[0].kind).contains("rival-history"));

    let durable = store.get(&id()).await.unwrap().unwrap();
    assert_eq!(durable.runtime_id, None);
    assert_eq!(durable.context_ref.as_deref(), Some("rival-context"));
    assert_eq!(durable.session.last_seq, 1);
    assert_eq!(durable.session.status, SessionStatus::Idle);
    assert_eq!(durable.rotation_claim.as_deref(), Some("rotation-stable"));
}

#[tokio::test]
async fn compatibility_runtime_clear_refuses_successor_authority() {
    enum SuccessorChange {
        Status,
        RotationClaim,
        Runtime,
    }

    for (case, change) in [
        ("status", SuccessorChange::Status),
        ("rotation claim", SuccessorChange::RotationClaim),
        ("runtime", SuccessorChange::Runtime),
    ] {
        let store = Arc::new(FakeStore::default());
        store.disable_coordination();
        let mut claimed = record();
        claimed.session.status = SessionStatus::Idle;
        claimed.session.recovery_state = RecoveryState::Idle;
        claimed.runtime_id = Some("runtime-observed".into());
        claimed.rotation_claim = Some("rotation-observed".into());
        store.seed(&claimed).await.expect("seed claimed runtime");

        let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
        let mut successor = claimed.clone();
        successor.context_ref = Some(format!("successor-context-{case}"));
        match change {
            SuccessorChange::Status => successor.session.status = SessionStatus::Ready,
            SuccessorChange::RotationClaim => {
                successor.rotation_claim = Some("rotation-successor".into())
            }
            SuccessorChange::Runtime => successor.runtime_id = Some("runtime-successor".into()),
        }
        store
            .commit_session_idempotent(
                &format!("compatibility-runtime-successor-{case}"),
                &successor,
                &[],
            )
            .await
            .unwrap_or_else(|error| panic!("{case} successor commits: {error}"));

        let mut intended = claimed.clone();
        intended.runtime_id = None;
        let error = service
            .clear_compatibility_runtime_durably(&mut intended, &claimed)
            .await
            .expect_err("successor authority rejects the stale runtime clear");
        assert!(
            matches!(error, StoreError::RevisionConflict { .. }),
            "{case} successor returned {error:?}"
        );

        let durable = store.get(&id()).await.unwrap().unwrap();
        assert_eq!(
            durable.session.status, successor.session.status,
            "{case} successor status"
        );
        assert_eq!(
            durable.rotation_claim, successor.rotation_claim,
            "{case} successor claim"
        );
        assert_eq!(
            durable.runtime_id, successor.runtime_id,
            "{case} successor runtime"
        );
        assert_eq!(
            durable.context_ref, successor.context_ref,
            "{case} unrelated state"
        );
        assert!(
            store.events_since(&id(), 0).await.unwrap().is_empty(),
            "{case} appends no lifecycle event"
        );
    }
}

#[tokio::test]
async fn two_services_keep_both_semantic_writes_after_a_revision_collision() {
    let store = Arc::new(FakeStore::default());
    let base = record();
    store.seed(&base).await.expect("seed record");
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
    store.seed(&base).await.expect("seed record");
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
    store.seed(&base).await.expect("seed record");
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
    store.seed(&base).await.expect("seed record");
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
    let mut base = record();
    base.skill_packs = crate::store::SkillPackState::Cleaned;
    store.seed(&base).await.expect("seed record");
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
async fn a_dropped_delete_waiter_still_stops_after_four_completed_outages() {
    let store = Arc::new(FaultStore::default());
    let base = record();
    store.inner.seed(&base).await.expect("seed record");
    store.delete_failures.store(5, Ordering::SeqCst);
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let lane = service.event_logs.lane(&id());

    let deletion = lane.enqueue_delete();
    drop(deletion);
    let successor = lane.enqueue_commit(
        super::CommitMode::UpdateExact,
        base,
        vec![said(0, "successor-after-dropped-delete-waiter")],
    );
    successor
        .complete
        .committed()
        .await
        .expect("the successor advances after the delete report boundary");
    wait_for_lane_idle(&lane).await;

    assert_eq!(
        store.delete_calls.load(Ordering::SeqCst),
        super::REPORT_AFTER_ATTEMPTS,
        "waiter cancellation does not turn delete into an unbounded retry"
    );
    assert_eq!(store.delete_failures.load(Ordering::SeqCst), 1);
    let durable = store.inner.get(&id()).await.unwrap().unwrap();
    assert_eq!(durable.session.last_seq, 1);
    let events = store.inner.events_since(&id(), 0).await.unwrap();
    assert_eq!(events.len(), 1);
    assert!(format!("{:?}", events[0].kind).contains("successor-after-dropped-delete-waiter"));
}

#[test]
fn an_unknown_delete_outcome_is_replayed_before_its_successor_on_a_new_runtime() {
    let origin = current_thread_runtime();
    let store = Arc::new(FaultStore::default());
    let mut base = record();
    base.skill_packs = crate::store::SkillPackState::Cleaned;
    origin
        .block_on(store.inner.seed(&base))
        .expect("seed record");
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let lane = service.event_logs.lane(&id());
    origin.block_on(async {
        lane.observe_record(&base);
        wait_for_lane_idle(&lane).await;
    });
    store.pause_delete_after_call.store(1, Ordering::SeqCst);

    let (deletion, successor) = origin.block_on(async {
        let deletion = lane.enqueue_delete();
        let successor = lane.enqueue_commit(
            super::CommitMode::UpdateInheriting,
            base,
            vec![said(0, "must-remain-after-replayed-delete")],
        );
        tokio::time::timeout(Duration::from_secs(2), store.delete_started.notified())
            .await
            .expect("the delete commits before its response is released");
        assert!(store.inner.get(&id()).await.unwrap().is_none());
        (deletion, successor)
    });

    drop(origin);
    assert!(
        !lane
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .running,
        "the unknown delete outcome releases the lane"
    );

    let replacement = current_thread_runtime();
    replacement.block_on(async {
        deletion
            .await
            .expect("the delete worker is replaced")
            .expect("the idempotent delete replay succeeds");
        assert!(matches!(
            successor
                .complete
                .committed()
                .await
                .expect_err("the successor runs only after deletion is observed"),
            StoreError::SessionDeleted { .. }
        ));
        wait_for_lane_idle(&lane).await;
    });

    assert_eq!(store.delete_calls.load(Ordering::SeqCst), 2);
    assert!(replacement
        .block_on(store.inner.get(&id()))
        .unwrap()
        .is_none());
}

#[test]
fn cancellation_during_the_fourth_delete_call_preserves_the_error_boundary() {
    let origin = current_thread_runtime();
    let store = Arc::new(FaultStore::default());
    let base = record();
    origin
        .block_on(store.inner.seed(&base))
        .expect("seed record");
    store.delete_failures.store(4, Ordering::SeqCst);
    store
        .pause_delete_before_call
        .store(super::REPORT_AFTER_ATTEMPTS, Ordering::SeqCst);
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let lane = service.event_logs.lane(&id());

    let (deletion, successor) = origin.block_on(async {
        let deletion = lane.enqueue_delete();
        let successor = lane.enqueue_commit(
            super::CommitMode::UpdateExact,
            base,
            vec![said(0, "successor-after-fourth-delete-boundary")],
        );
        tokio::time::timeout(Duration::from_secs(2), store.delete_started.notified())
            .await
            .expect("the fourth started delete is held before its result");
        assert_eq!(store.delete_calls.load(Ordering::SeqCst), 4);
        (deletion, successor)
    });

    drop(origin);
    assert!(
        !lane
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .running,
        "the interrupted fourth call releases the lane"
    );

    let replacement = current_thread_runtime();
    replacement.block_on(async {
        assert!(matches!(
            deletion
                .await
                .expect("the replacement delete worker reports")
                .expect_err("the fourth completed unavailable result is reported"),
            StoreError::Unavailable(_)
        ));
        successor
            .complete
            .committed()
            .await
            .expect("the queued successor advances after the reported delete failure");
        wait_for_lane_idle(&lane).await;
    });

    assert_eq!(store.delete_calls.load(Ordering::SeqCst), 5);
    let events = replacement
        .block_on(store.inner.events_since(&id(), 0))
        .unwrap();
    assert_eq!(events.len(), 1);
    assert!(format!("{:?}", events[0].kind).contains("successor-after-fourth-delete-boundary"));
}

#[tokio::test]
async fn an_old_receipt_never_replays_its_record_over_a_newer_record_only_write() {
    let store = FakeStore::default();
    let base = record();
    store.seed(&base).await.expect("seed record");

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
    store.seed(&update).await.expect("seed record");
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
async fn stale_state_can_only_attempt_cas_and_cannot_reset_the_revision() {
    let store = FakeStore::default();
    let base = record();
    store.seed(&base).await.expect("seed record");

    let mut winner = base.clone();
    winner.context_ref = Some("newer-context".into());
    let revision = store
        .commit_session_idempotent("newer-cas", &winner, &[])
        .await
        .expect("newer writer commits");
    assert_eq!(revision, 1);

    let mut stale = base;
    stale.session.status = SessionStatus::Idle;
    stale.session.recovery_state = RecoveryState::Idle;
    stale.session.stop_reason = Some(StopReason::UserTerminated);
    let error = store
        .commit_session_idempotent("stale-cas", &stale, &[])
        .await
        .expect_err("stale state cannot replace the durable record");
    assert!(matches!(
        error,
        StoreError::RevisionConflict {
            expected: 0,
            actual: 1
        }
    ));

    let durable = store.get(&id()).await.unwrap().unwrap();
    assert_eq!(durable.revision, 1);
    assert_eq!(durable.context_ref.as_deref(), Some("newer-context"));
    assert_eq!(durable.session.status, SessionStatus::Ready);
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
        .seed(&legacy)
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
    store.seed(&terminal).await.expect("seed terminal record");

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
