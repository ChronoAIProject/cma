use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

use layer3_adapter::ConversationInput;
use tokio::sync::{Mutex, Notify};

use crate::conversation_support::conversing;
use crate::coordination::{
    RuntimeAuthority, RuntimeAuthoritySwitch, RuntimeCandidate, RuntimeReservation,
    SessionBoundary, SessionInteractionLeaseRequest, SessionLease, SessionLeaseRequest,
    SessionOperation,
};
use crate::environment::AgentConfiguration;
use crate::event::{Event, EventKind, StoredEvent};
use crate::ids::{KernelId, SessionId, SubjectId};
use crate::options::SessionMode;
use crate::provision_support::RuntimeInteractionDeliveryLoss;
use crate::session::{RecoveryState, Session, SessionStatus};
use crate::store::{Page, SessionListQuery, SessionRecord, SessionStore, StoreError};
use crate::test_support::{
    await_settled, environments_defaulting_to, kernel, service_over,
    service_over_with_environments, spec, user, FakeAdapter, FakeStore,
};
use crate::{AgentOptions, CoreError, Reserved, TerminalGeometry};

struct WakeFlag(AtomicBool);

impl Wake for WakeFlag {
    fn wake(self: Arc<Self>) {
        self.0.store(true, Ordering::SeqCst);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct InteractionStore {
    inner: Arc<FakeStore>,
    compatibility: AtomicBool,
    release_failures: AtomicUsize,
    unknown_release_results: AtomicUsize,
    stall_release_after_commit: AtomicBool,
    release_response_stalled: AtomicBool,
    release_committed: Notify,
    release_may_return: Notify,
    interaction_released: Notify,
    stall_prompt_commit: AtomicBool,
    prompt_commit_stalled: Notify,
    prompt_commit_may_return: Notify,
    renewal_failures: AtomicUsize,
    acquisitions: Mutex<Vec<SessionInteractionLeaseRequest>>,
    renewals: Mutex<Vec<(SessionLease, Duration)>>,
    releases: Mutex<Vec<SessionLease>>,
}

impl InteractionStore {
    fn compatibility() -> Self {
        Self {
            compatibility: AtomicBool::new(true),
            ..Self::default()
        }
    }

    fn fail_renewals(&self, count: usize) {
        self.renewal_failures.store(count, Ordering::SeqCst);
    }

    fn fail_releases(&self, count: usize) {
        self.release_failures.store(count, Ordering::SeqCst);
    }

    fn obscure_release_success(&self) {
        self.unknown_release_results.store(1, Ordering::SeqCst);
    }

    fn stall_next_release_after_commit(&self) {
        self.stall_release_after_commit
            .store(true, Ordering::SeqCst);
    }

    fn stall_next_prompt_commit(&self) {
        assert!(!self.stall_prompt_commit.swap(true, Ordering::SeqCst));
    }

    async fn wait_until_prompt_commit_stalled(&self) {
        tokio::time::timeout(
            Duration::from_secs(2),
            self.prompt_commit_stalled.notified(),
        )
        .await
        .expect("the receipted prompt reached its durable commit");
    }

    fn resume_prompt_commit(&self) {
        self.prompt_commit_may_return.notify_one();
    }

    async fn wait_until_interaction_released(&self) {
        tokio::time::timeout(Duration::from_secs(2), self.interaction_released.notified())
            .await
            .expect("the interaction lease was released");
    }

    fn consume(counter: &AtomicUsize) -> bool {
        counter
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
    }
}

#[async_trait::async_trait]
impl SessionStore for InteractionStore {
    async fn ping(&self) -> Result<(), StoreError> {
        self.inner.ping().await
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
        self.inner.get(id).await
    }

    async fn runtime_authority(
        &self,
        id: &SessionId,
    ) -> Result<Option<RuntimeAuthority>, StoreError> {
        if self.compatibility.load(Ordering::SeqCst) {
            Err(StoreError::CoordinationDisabled)
        } else {
            self.inner.runtime_authority(id).await
        }
    }

    async fn acquire_session_lease(
        &self,
        request: &SessionLeaseRequest,
    ) -> Result<SessionLease, StoreError> {
        self.inner.acquire_session_lease(request).await
    }

    async fn acquire_interaction_lease(
        &self,
        request: &SessionInteractionLeaseRequest,
    ) -> Result<SessionLease, StoreError> {
        self.acquisitions.lock().await.push(request.clone());
        self.inner.acquire_interaction_lease(request).await
    }

    async fn renew_session_lease(
        &self,
        lease: &SessionLease,
        ttl: Duration,
    ) -> Result<(), StoreError> {
        self.inner.renew_session_lease(lease, ttl).await
    }

    async fn renew_interaction_lease(
        &self,
        lease: &SessionLease,
        ttl: Duration,
    ) -> Result<(), StoreError> {
        self.renewals.lock().await.push((lease.clone(), ttl));
        if Self::consume(&self.renewal_failures) {
            return Err(StoreError::Unavailable(
                "interaction renewal unavailable".into(),
            ));
        }
        self.inner.renew_interaction_lease(lease, ttl).await
    }

    async fn advance_session_lease(
        &self,
        lease: &mut SessionLease,
        boundary: &SessionBoundary,
    ) -> Result<(), StoreError> {
        self.inner.advance_session_lease(lease, boundary).await
    }

    async fn release_session_lease(&self, lease: &SessionLease) -> Result<(), StoreError> {
        if lease.operation != SessionOperation::Interact {
            return self.inner.release_session_lease(lease).await;
        }
        self.releases.lock().await.push(lease.clone());
        if Self::consume(&self.release_failures) {
            return Err(StoreError::Unavailable(
                "interaction release unavailable".into(),
            ));
        }
        let released = self.inner.release_session_lease(lease).await;
        if released.is_ok() {
            self.interaction_released.notify_one();
        }
        if released.is_ok()
            && self
                .stall_release_after_commit
                .swap(false, Ordering::SeqCst)
        {
            self.release_response_stalled.store(true, Ordering::SeqCst);
            self.release_committed.notify_one();
            self.release_may_return.notified().await;
            self.release_response_stalled.store(false, Ordering::SeqCst);
        }
        if released.is_ok() && Self::consume(&self.unknown_release_results) {
            return Err(StoreError::Unavailable(
                "interaction release result was lost".into(),
            ));
        }
        released
    }

    async fn reserve_runtime_generation(
        &self,
        lease: &SessionLease,
        token: &str,
    ) -> Result<RuntimeReservation, StoreError> {
        self.inner.reserve_runtime_generation(lease, token).await
    }

    async fn register_runtime_candidate(
        &self,
        lease: &SessionLease,
        reservation: &RuntimeReservation,
        runtime_id: &str,
    ) -> Result<RuntimeCandidate, StoreError> {
        self.inner
            .register_runtime_candidate(lease, reservation, runtime_id)
            .await
    }

    async fn switch_runtime_authority(
        &self,
        lease: &mut SessionLease,
        candidate: &RuntimeCandidate,
    ) -> Result<RuntimeAuthoritySwitch, StoreError> {
        self.inner.switch_runtime_authority(lease, candidate).await
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

    async fn commit_session_idempotent(
        &self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError> {
        if events
            .iter()
            .any(|event| matches!(&event.kind, EventKind::TurnStarted))
            && self.stall_prompt_commit.swap(false, Ordering::SeqCst)
        {
            self.prompt_commit_stalled.notify_one();
            self.prompt_commit_may_return.notified().await;
        }
        self.inner
            .commit_session_idempotent(write_id, record, events)
            .await
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

fn record() -> SessionRecord {
    let mut record = SessionRecord::new(Session {
        id: SessionId("sesn_runtime_interaction".into()),
        owner: SubjectId("subject-runtime-interaction".into()),
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
        created_at: "2026-08-26T00:00:00Z".into(),
    });
    record.runtime_id = Some("runtime-original".into());
    record
}

async fn promote_runtime(store: &InteractionStore) -> SessionRecord {
    let current = store.get(&record().session.id).await.unwrap().unwrap();
    let authority = store
        .runtime_authority(&current.session.id)
        .await
        .unwrap()
        .unwrap();
    let request = SessionLeaseRequest::new(
        current.session.id.clone(),
        "runtime-promoter",
        SessionBoundary::for_record(&current, authority.generation),
        SessionOperation::Handover,
        Duration::from_secs(30),
    );
    let mut lease = store.acquire_session_lease(&request).await.unwrap();
    let reservation = store
        .reserve_runtime_generation(&lease, "interaction-runtime-generation")
        .await
        .unwrap();
    let candidate = store
        .register_runtime_candidate(&lease, &reservation, "runtime-generation-one")
        .await
        .unwrap();
    let switched = store
        .switch_runtime_authority(&mut lease, &candidate)
        .await
        .unwrap();
    assert_eq!(switched.boundary.authority.generation, 1);
    store.release_session_lease(&lease).await.unwrap();
    store.get(&current.session.id).await.unwrap().unwrap()
}

async fn wait_for_releases(store: &InteractionStore, expected: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if store.releases.lock().await.len() >= expected {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("interaction release attempts completed");
}

async fn cancellation_keeps_action_owned(store: Arc<InteractionStore>) {
    store.inner.seed(&record()).await.unwrap();
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = store.get(&record().session.id).await.unwrap().unwrap();
    let (started, is_started) = tokio::sync::oneshot::channel();
    let (finish, may_finish) = tokio::sync::oneshot::channel();
    let (completed, is_completed) = tokio::sync::oneshot::channel();
    let caller = tokio::spawn({
        let service = Arc::clone(&service);
        async move {
            service
                .run_runtime_interaction_at(&durable, move |authority| async move {
                    let boundary = authority.ensure_delivery_allowed()?.clone();
                    let durably_fenced = authority.is_durably_fenced();
                    let _ = started.send((boundary, durably_fenced));
                    let _ = may_finish.await;
                    let _ = completed.send(());
                    Ok::<_, CoreError>(())
                })
                .await
        }
    });

    let (boundary, durably_fenced) = is_started.await.expect("the owned action started");
    assert_eq!(
        boundary.authority.runtime_id.as_deref(),
        Some("runtime-original")
    );
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    finish.send(()).unwrap();
    is_completed.await.expect("the owned action completed");

    let compatibility = store.compatibility.load(Ordering::SeqCst);
    assert_eq!(durably_fenced, !compatibility);
    if compatibility {
        assert!(store.releases.lock().await.is_empty());
    } else {
        wait_for_releases(&store, 1).await;
    }
}

#[tokio::test]
async fn caller_cancellation_does_not_cancel_a_durable_interaction() {
    cancellation_keeps_action_owned(Arc::new(InteractionStore::default())).await;
}

#[tokio::test]
async fn caller_cancellation_does_not_cancel_a_compatibility_interaction() {
    cancellation_keeps_action_owned(Arc::new(InteractionStore::compatibility())).await;
}

#[tokio::test]
async fn retained_result_reports_send_failure_after_caller_cancellation() {
    let store = Arc::new(InteractionStore::default());
    store.inner.seed(&record()).await.unwrap();
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = store.get(&record().session.id).await.unwrap().unwrap();
    let (started, is_started) = tokio::sync::oneshot::channel();
    let (finish, may_finish) = tokio::sync::oneshot::channel();
    let (lost, mut losses) = tokio::sync::mpsc::unbounded_channel();
    let caller = tokio::spawn({
        let service = Arc::clone(&service);
        async move {
            service
                .run_retained_runtime_interaction_at(
                    &durable,
                    move |authority, _interaction| async move {
                        authority.ensure_delivery_allowed()?;
                        let _ = started.send(());
                        let _ = may_finish.await;
                        Ok::<_, CoreError>("completed after cancellation")
                    },
                    move |reason, result, _interaction| {
                        let _ = lost.send((reason, result.is_ok()));
                    },
                )
                .await
        }
    });

    is_started.await.expect("the retained action started");
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    finish.send(()).unwrap();
    let (reason, succeeded) = tokio::time::timeout(Duration::from_secs(2), losses.recv())
        .await
        .expect("the undelivered hook runs")
        .expect("the loss is reported");
    assert_eq!(reason, RuntimeInteractionDeliveryLoss::SendFailed);
    assert!(succeeded);
    wait_for_releases(&store, 1).await;
    assert_eq!(store.releases.lock().await.len(), 1);
}

#[tokio::test]
async fn retained_success_sent_but_dropped_before_poll_reports_unread() {
    let store = Arc::new(InteractionStore::default());
    store.inner.seed(&record()).await.unwrap();
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = store.get(&record().session.id).await.unwrap().unwrap();
    let (completed, is_completed) = tokio::sync::oneshot::channel();
    let (lost, losses) = std::sync::mpsc::sync_channel(1);
    let mut caller = Box::pin(service.run_retained_runtime_interaction_at(
        &durable,
        move |authority, _interaction| async move {
            authority.ensure_delivery_allowed()?;
            let _ = completed.send(());
            Ok::<_, CoreError>("buffered success")
        },
        move |reason, result, interaction| {
            let invalidated = interaction.authority().ensure_delivery_allowed().is_err();
            let _ = lost.send((reason, result.is_ok(), invalidated));
        },
    ));
    let woken = Arc::new(WakeFlag(AtomicBool::new(false)));
    let waker = Waker::from(Arc::clone(&woken));
    let mut context = Context::from_waker(&waker);
    assert!(matches!(caller.as_mut().poll(&mut context), Poll::Pending));

    is_completed.await.expect("the retained action completed");
    tokio::task::yield_now().await;
    assert!(
        woken.0.load(Ordering::SeqCst),
        "the completed result wakes its unpolled caller"
    );
    drop(caller);

    let (reason, succeeded, invalidated) = losses
        .recv_timeout(Duration::from_secs(2))
        .expect("dropping the buffered delivery runs its hook synchronously");
    assert_eq!(reason, RuntimeInteractionDeliveryLoss::Unread);
    assert!(succeeded);
    assert!(invalidated, "caller cancellation precedes the unread hook");
    wait_for_releases(&store, 1).await;
}

#[tokio::test]
async fn retained_owner_clones_queue_one_exact_release() {
    let store = Arc::new(InteractionStore::default());
    store.inner.seed(&record()).await.unwrap();
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = store.get(&record().session.id).await.unwrap().unwrap();

    service
        .run_retained_runtime_interaction_at(
            &durable,
            |authority, interaction| async move {
                authority.ensure_delivery_allowed()?;
                let first = interaction.clone();
                let second = interaction.clone();
                drop(first);
                drop(second);
                Ok::<_, CoreError>(())
            },
            |_reason, _result, _interaction| panic!("an observed result is not undelivered"),
        )
        .await
        .unwrap();

    wait_for_releases(&store, 1).await;
    tokio::task::yield_now().await;
    let releases = store.releases.lock().await;
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0].operation, SessionOperation::Interact);
}

#[test]
fn retained_owner_resumes_and_releases_after_origin_runtime_replacement() {
    let origin = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("origin runtime builds");
    let store = Arc::new(InteractionStore::default());
    origin.block_on(store.inner.seed(&record())).unwrap();
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = origin
        .block_on(store.get(&record().session.id))
        .unwrap()
        .unwrap();
    let (retained, retained_result) = std::sync::mpsc::sync_channel(1);
    let mut caller = Box::pin(service.run_retained_runtime_interaction_at(
        &durable,
        move |authority, interaction| async move {
            authority.ensure_delivery_allowed()?;
            assert!(
                retained.send(interaction.clone()).is_ok(),
                "the replacement-runtime owner is received"
            );
            std::future::pending::<Result<&'static str, CoreError>>().await
        },
        |_reason, _result, _interaction| {},
    ));
    let interaction = origin.block_on(async {
        loop {
            std::future::poll_fn(|context| {
                assert!(matches!(caller.as_mut().poll(context), Poll::Pending));
                Poll::Ready(())
            })
            .await;
            if let Ok(interaction) = retained_result.try_recv() {
                return interaction;
            }
            tokio::task::yield_now().await;
        }
    });
    let held = origin
        .block_on(store.inner.active_session_lease(&durable.session.id))
        .expect("the retained interaction owns its exact lease");
    drop(origin);

    let replacement = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("replacement runtime builds");
    let resumed = replacement
        .block_on(interaction.resume_before_delivery())
        .expect("the retained exact lease renews on the replacement runtime");
    resumed
        .ensure_delivery_allowed()
        .expect("the retained authority remains usable");
    let renewals = replacement.block_on(store.renewals.lock());
    assert_eq!(
        renewals.as_slice(),
        &[(held.clone(), Duration::from_secs(30))]
    );
    drop(renewals);
    drop(replacement);

    drop(caller);
    drop(interaction);
    assert!(
        store
            .releases
            .try_lock()
            .expect("release log is unlocked")
            .is_empty(),
        "dropping outside a runtime only retains service-owned release"
    );

    let successor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("successor runtime builds");
    successor.block_on(async {
        service
            .run_runtime_interaction_at(&durable, |authority| async move {
                authority.ensure_delivery_allowed()?;
                Ok::<_, CoreError>(())
            })
            .await
            .expect("a successor drains the orphaned release and acquires");
        wait_for_releases(&store, 2).await;
    });
    let releases = successor.block_on(store.releases.lock());
    assert_eq!(releases.len(), 2);
    assert_eq!(releases[0], held);
    assert_ne!(releases[0].token, releases[1].token);
}

#[tokio::test(start_paused = true)]
async fn cancelled_prompt_keeps_renewing_until_receipted_commit_settles() {
    let store = Arc::new(InteractionStore::default());
    let adapter = Arc::new(FakeAdapter {
        capabilities: conversing(),
        ..FakeAdapter::default()
    });
    let profiles = environments_defaulting_to(AgentConfiguration {
        kernel: kernel(),
        mode: SessionMode::Conversation,
        ..Default::default()
    });
    let service = service_over_with_environments(store.clone(), Arc::clone(&adapter), profiles);
    let alice = user("cancelled-prompt-finalization-owner");
    let created = service
        .create_session(&alice, spec())
        .await
        .expect("created");
    await_settled(&service, &alice, &created.id).await;
    store.stall_next_prompt_commit();

    let caller = tokio::spawn({
        let service = Arc::clone(&service);
        let alice = alice.clone();
        let id = created.id.clone();
        async move {
            service
                .send_prompt(&alice, &id, "retain the finalization fence".into())
                .await
        }
    });
    store.wait_until_prompt_commit_stalled().await;
    let held = store
        .inner
        .active_session_lease(&created.id)
        .await
        .expect("the prompt retains its interaction lease");
    assert_eq!(held.operation, SessionOperation::Interact);
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());

    for expected_renewals in 1..=6 {
        tokio::time::advance(Duration::from_secs(11)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            store.renewals.lock().await.len(),
            expected_renewals,
            "caller cancellation must not stop successful lease renewal"
        );
        assert_eq!(
            store.inner.active_session_lease(&created.id).await,
            Some(held.clone()),
            "the same lease remains authoritative beyond its original TTL"
        );
    }

    assert!(matches!(
        service.interrupt_turn(&alice, &created.id).await,
        Err(CoreError::RuntimeUnavailable(_))
    ));
    let sent = adapter
        .conversation
        .lock()
        .await
        .as_ref()
        .expect("the conversation remains live")
        .sent()
        .await;
    assert!(matches!(sent.as_slice(), [ConversationInput::Prompt(_)]));

    store.resume_prompt_commit();
    store.wait_until_interaction_released().await;
    assert!(store
        .inner
        .active_session_lease(&created.id)
        .await
        .is_none());
    let events = service
        .events_since(&alice, &created.id, None)
        .await
        .expect("the finalized prompt history is readable");
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        EventKind::OwnerMessage { content } if content == "retain the finalization fence"
    )));

    service
        .interrupt_turn(&alice, &created.id)
        .await
        .expect("a successor proceeds after prompt finalization releases its lease");
    store.wait_until_interaction_released().await;
    let sent = adapter
        .conversation
        .lock()
        .await
        .as_ref()
        .expect("the conversation remains live")
        .sent()
        .await;
    assert!(matches!(
        sent.as_slice(),
        [ConversationInput::Prompt(_), ConversationInput::Cancel]
    ));
}

#[tokio::test]
async fn action_success_survives_an_unknown_release_and_retries_the_exact_lease() {
    let store = Arc::new(InteractionStore::default());
    store.inner.seed(&record()).await.unwrap();
    store.obscure_release_success();
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = store.get(&record().session.id).await.unwrap().unwrap();
    let runs = Arc::new(AtomicUsize::new(0));

    let result = service
        .run_runtime_interaction_at(&durable, {
            let runs = Arc::clone(&runs);
            move |authority| async move {
                authority.ensure_delivery_allowed()?;
                runs.fetch_add(1, Ordering::SeqCst);
                Ok::<_, CoreError>("delivered")
            }
        })
        .await;

    assert_eq!(result.unwrap(), "delivered");
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    wait_for_releases(&store, 2).await;
    let releases = store.releases.lock().await;
    assert_eq!(releases.len(), 2);
    assert_eq!(releases[0], releases[1]);
}

#[tokio::test]
async fn action_success_survives_an_unavailable_release() {
    let store = Arc::new(InteractionStore::default());
    store.inner.seed(&record()).await.unwrap();
    store.fail_releases(2);
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = store.get(&record().session.id).await.unwrap().unwrap();

    let result = service
        .run_runtime_interaction_at(&durable, |authority| async move {
            authority.ensure_delivery_allowed()?;
            Ok::<_, CoreError>("known-success")
        })
        .await;

    assert_eq!(result.unwrap(), "known-success");
    wait_for_releases(&store, 2).await;
    let releases = store.releases.lock().await;
    assert_eq!(releases[0], releases[1]);
}

#[tokio::test]
async fn known_result_precedes_a_stalled_release_and_a_successor_can_proceed() {
    let store = Arc::new(InteractionStore::default());
    store.inner.seed(&record()).await.unwrap();
    store.stall_next_release_after_commit();
    let first_service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let second_service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = store.get(&record().session.id).await.unwrap().unwrap();

    let first = tokio::spawn({
        let service = Arc::clone(&first_service);
        let durable = durable.clone();
        async move {
            service
                .run_runtime_interaction_at(&durable, |authority| async move {
                    authority.ensure_delivery_allowed()?;
                    Ok::<_, CoreError>("first-delivered")
                })
                .await
        }
    });
    let first_result = tokio::time::timeout(Duration::from_secs(2), first)
        .await
        .expect("the known action result does not wait for release")
        .expect("the first caller task completes")
        .expect("the first interaction succeeds");
    assert_eq!(first_result, "first-delivered");

    tokio::time::timeout(Duration::from_secs(2), store.release_committed.notified())
        .await
        .expect("the first release commits before its response stalls");
    assert!(store.release_response_stalled.load(Ordering::SeqCst));

    let successor_runs = Arc::new(AtomicUsize::new(0));
    let successor_result = tokio::time::timeout(
        Duration::from_secs(2),
        second_service.run_runtime_interaction_at(&durable, {
            let successor_runs = Arc::clone(&successor_runs);
            move |authority| async move {
                authority.ensure_delivery_allowed()?;
                successor_runs.fetch_add(1, Ordering::SeqCst);
                Ok::<_, CoreError>("successor-delivered")
            }
        }),
    )
    .await
    .expect("a successor does not wait for the first release response")
    .expect("the successor interaction succeeds");
    assert_eq!(successor_result, "successor-delivered");
    assert_eq!(successor_runs.load(Ordering::SeqCst), 1);
    assert!(store.release_response_stalled.load(Ordering::SeqCst));

    wait_for_releases(&store, 2).await;
    let releases = store.releases.lock().await;
    assert_ne!(releases[0].token, releases[1].token);
    drop(releases);

    store.release_may_return.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        while store.release_response_stalled.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the first release response resumes");
}

#[tokio::test]
async fn active_action_excludes_another_service_and_receives_the_exact_generation() {
    let store = Arc::new(InteractionStore::default());
    store.inner.seed(&record()).await.unwrap();
    let durable = promote_runtime(&store).await;
    let first = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let second = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let (started, is_started) = tokio::sync::oneshot::channel();
    let (finish, may_finish) = tokio::sync::oneshot::channel();
    let first_action = tokio::spawn({
        let first = Arc::clone(&first);
        let durable = durable.clone();
        async move {
            first
                .run_runtime_interaction_at(&durable, move |authority| async move {
                    let boundary = authority.ensure_delivery_allowed()?.clone();
                    let _ = started.send(boundary);
                    let _ = may_finish.await;
                    Ok::<_, CoreError>(())
                })
                .await
        }
    });

    let boundary = is_started.await.expect("the first action holds the lease");
    assert_eq!(boundary.authority.generation, 1);
    assert_eq!(
        boundary.authority.runtime_id.as_deref(),
        Some("runtime-generation-one")
    );
    let second_runs = Arc::new(AtomicUsize::new(0));
    let blocked = second
        .run_runtime_interaction_at(&durable, {
            let second_runs = Arc::clone(&second_runs);
            move |authority| async move {
                authority.ensure_delivery_allowed()?;
                second_runs.fetch_add(1, Ordering::SeqCst);
                Ok::<_, CoreError>(())
            }
        })
        .await;
    assert!(matches!(blocked, Err(CoreError::RuntimeUnavailable(_))));
    assert_eq!(second_runs.load(Ordering::SeqCst), 0);

    let acquisitions = store.acquisitions.lock().await;
    assert_eq!(acquisitions[0].ttl, Duration::from_secs(30));
    drop(acquisitions);
    finish.send(()).unwrap();
    first_action.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn renewal_survives_an_action_owned_status_transition() {
    let store = Arc::new(InteractionStore::default());
    store.inner.seed(&record()).await.unwrap();
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = store.get(&record().session.id).await.unwrap().unwrap();
    let (changed, is_changed) = tokio::sync::oneshot::channel();
    let (finish, may_finish) = tokio::sync::oneshot::channel();
    let runner = tokio::spawn({
        let service = Arc::clone(&service);
        let store = Arc::clone(&store);
        let mut changed_record = durable.clone();
        async move {
            service
                .run_runtime_interaction_at(&durable, move |authority| async move {
                    authority.ensure_delivery_allowed()?;
                    changed_record.session.status = SessionStatus::Idle;
                    store
                        .commit_session_idempotent(
                            "runner-interaction-status-drift",
                            &changed_record,
                            &[],
                        )
                        .await
                        .unwrap();
                    let _ = changed.send(());
                    let _ = may_finish.await;
                    Ok::<_, CoreError>(())
                })
                .await
        }
    });

    is_changed.await.unwrap();
    tokio::time::advance(Duration::from_secs(11)).await;
    tokio::task::yield_now().await;
    assert_eq!(store.renewals.lock().await.len(), 1);
    assert!(!runner.is_finished());
    finish.send(()).unwrap();
    runner.await.unwrap().unwrap();
    let renewals = store.renewals.lock().await;
    assert_eq!(renewals[0].1, Duration::from_secs(30));
}

#[tokio::test(start_paused = true)]
async fn transient_renewal_failure_is_retried_without_canceling_the_action() {
    let store = Arc::new(InteractionStore::default());
    store.inner.seed(&record()).await.unwrap();
    store.fail_renewals(1);
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = store.get(&record().session.id).await.unwrap().unwrap();
    let (started, is_started) = tokio::sync::oneshot::channel();
    let (finish, may_finish) = tokio::sync::oneshot::channel();
    let runner = tokio::spawn({
        let service = Arc::clone(&service);
        async move {
            service
                .run_runtime_interaction_at(&durable, move |authority| async move {
                    authority.ensure_delivery_allowed()?;
                    let _ = started.send(());
                    let _ = may_finish.await;
                    Ok::<_, CoreError>("settled")
                })
                .await
        }
    });

    is_started.await.unwrap();
    tokio::time::advance(Duration::from_secs(10)).await;
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    let renewals = store.renewals.lock().await;
    assert_eq!(renewals.len(), 2);
    assert_eq!(renewals[0].0, renewals[1].0);
    drop(renewals);
    assert!(!runner.is_finished());
    finish.send(()).unwrap();
    assert_eq!(runner.await.unwrap().unwrap(), "settled");
}

#[tokio::test(start_paused = true)]
async fn confirmed_renewal_loss_does_not_drop_a_known_action_success() {
    let store = Arc::new(InteractionStore::default());
    store.inner.seed(&record()).await.unwrap();
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = store.get(&record().session.id).await.unwrap().unwrap();
    let (started, is_started) = tokio::sync::oneshot::channel();
    let (finish, may_finish) = tokio::sync::oneshot::channel();
    let runner = tokio::spawn({
        let service = Arc::clone(&service);
        async move {
            service
                .run_runtime_interaction_at(&durable, move |authority| async move {
                    authority.ensure_delivery_allowed()?;
                    let _ = started.send(());
                    let _ = may_finish.await;
                    Ok::<_, CoreError>("committed")
                })
                .await
        }
    });

    is_started.await.unwrap();
    store
        .inner
        .sessions
        .lock()
        .await
        .get_mut(&record().session.id.0)
        .unwrap()
        .runtime_id = Some("runtime-changed".into());
    tokio::time::advance(Duration::from_secs(10)).await;
    tokio::task::yield_now().await;
    assert_eq!(store.renewals.lock().await.len(), 1);
    assert!(!runner.is_finished());
    finish.send(()).unwrap();
    assert_eq!(runner.await.unwrap().unwrap(), "committed");
}

#[tokio::test(start_paused = true)]
async fn a_receipted_delivery_finishes_required_local_state_after_authority_expires() {
    let store = Arc::new(InteractionStore::default());
    store.inner.seed(&record()).await.unwrap();
    store.fail_renewals(usize::MAX);
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = store.get(&record().session.id).await.unwrap().unwrap();
    let deliveries = Arc::new(AtomicUsize::new(0));
    let finalizations = Arc::new(AtomicUsize::new(0));
    let (receipted, receipt_observed) = tokio::sync::oneshot::channel();
    let (finalize, may_finalize) = tokio::sync::oneshot::channel();
    let runner = tokio::spawn({
        let service = Arc::clone(&service);
        let deliveries = Arc::clone(&deliveries);
        let finalizations = Arc::clone(&finalizations);
        async move {
            service
                .run_runtime_interaction_at(&durable, move |authority| async move {
                    authority.ensure_delivery_allowed()?;
                    deliveries.fetch_add(1, Ordering::SeqCst);
                    let _ = receipted.send(authority.clone());
                    let _ = may_finalize.await;
                    // A successful external receipt makes this local enqueue
                    // mandatory; a second deadline check could strand it.
                    finalizations.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, CoreError>("finalized")
                })
                .await
        }
    });

    let authority = receipt_observed
        .await
        .expect("the external delivery returned a receipt");
    assert_eq!(deliveries.load(Ordering::SeqCst), 1);
    tokio::time::advance(Duration::from_secs(30)).await;
    tokio::task::yield_now().await;
    assert!(matches!(
        authority.ensure_delivery_allowed(),
        Err(CoreError::RuntimeUnavailable(_))
    ));
    assert!(!runner.is_finished());

    finalize.send(()).unwrap();
    assert_eq!(runner.await.unwrap().unwrap(), "finalized");
    assert_eq!(finalizations.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn an_action_too_close_to_expiry_is_refused_at_its_mutation_boundary() {
    let store = Arc::new(InteractionStore::default());
    store.inner.seed(&record()).await.unwrap();
    store.fail_renewals(usize::MAX);
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = store.get(&record().session.id).await.unwrap().unwrap();
    let mutations = Arc::new(AtomicUsize::new(0));
    let (started, is_started) = tokio::sync::oneshot::channel();
    let (enter_gate, gate_opened) = tokio::sync::oneshot::channel();
    let runner = tokio::spawn({
        let service = Arc::clone(&service);
        let mutations = Arc::clone(&mutations);
        async move {
            service
                .run_runtime_interaction_at(&durable, move |authority| async move {
                    let _ = started.send(());
                    let _ = gate_opened.await;
                    authority.ensure_delivery_allowed()?;
                    mutations.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, CoreError>(())
                })
                .await
        }
    });

    is_started.await.unwrap();
    tokio::time::advance(Duration::from_secs(26)).await;
    tokio::task::yield_now().await;
    enter_gate.send(()).unwrap();
    assert!(matches!(
        runner.await.unwrap(),
        Err(CoreError::RuntimeUnavailable(_))
    ));
    assert_eq!(mutations.load(Ordering::SeqCst), 0);
}
