use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

use layer3_adapter::{ConversationInput, KernelAdapter};
use tokio::sync::{Mutex, Notify};

use crate::conversation::ApprovalDecision;
use crate::conversation_support::{asking, conversing};
use crate::coordination::{
    ConversationLink, ConversationOpeningLease, ConversationOpeningLeaseRequest,
    ConversationWriteAuthority, RuntimeAuthority, RuntimeAuthoritySwitch, RuntimeCandidate,
    RuntimeReservation, SessionBoundary, SessionInteractionLeaseRequest, SessionLease,
    SessionLeaseRequest, SessionOperation,
};
use crate::environment::AgentConfiguration;
use crate::event::{Event, EventKind, StoredEvent};
use crate::ids::{KernelId, SessionId, SubjectId};
use crate::options::SessionMode;
use crate::provision_support::{RetainedRuntimeInteraction, RuntimeInteractionDeliveryLoss};
use crate::session::{RecoveryState, Session, SessionStatus};
use crate::store::{Page, SessionListQuery, SessionRecord, SessionStore, StoreError};
use crate::test_support::{
    await_settled, environments_defaulting_to, kernel, service_over,
    service_over_with_environments, spec, user, FakeAdapter, FakeStore, NullScreen,
};
use crate::{AgentOptions, CoreError, Reserved, SessionService, TerminalGeometry};

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
    stall_approval_commit: AtomicBool,
    approval_commit_stalled: Notify,
    approval_commit_may_return: Notify,
    approval_request_committed: Notify,
    renewal_failures: AtomicUsize,
    renewal_unavailable: Notify,
    unknown_renewal_results: AtomicUsize,
    detach_next_renewal: AtomicBool,
    detached_renewal_dispatched: AtomicBool,
    detached_renewal_may_commit: Arc<Notify>,
    detached_renewal_committed: Arc<AtomicBool>,
    stall_acquisition_after_commit: AtomicBool,
    acquisition_committed: Notify,
    unknown_acquisition_results: AtomicUsize,
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

    fn obscure_renewal_successes(&self, count: usize) {
        self.unknown_renewal_results.store(count, Ordering::SeqCst);
    }

    fn detach_next_renewal(&self) {
        assert!(!self.detach_next_renewal.swap(true, Ordering::SeqCst));
        self.detached_renewal_dispatched
            .store(false, Ordering::SeqCst);
        self.detached_renewal_committed
            .store(false, Ordering::SeqCst);
    }

    fn allow_detached_renewal_to_commit(&self) {
        self.detached_renewal_may_commit.notify_one();
    }

    fn stall_next_acquisition_after_commit(&self) {
        assert!(!self
            .stall_acquisition_after_commit
            .swap(true, Ordering::SeqCst));
    }

    fn obscure_acquisition_successes(&self, count: usize) {
        self.unknown_acquisition_results
            .store(count, Ordering::SeqCst);
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

    fn stall_next_approval_commit(&self) {
        assert!(!self.stall_approval_commit.swap(true, Ordering::SeqCst));
    }

    async fn wait_until_approval_commit_stalled(&self) {
        tokio::time::timeout(
            Duration::from_secs(2),
            self.approval_commit_stalled.notified(),
        )
        .await
        .expect("the receipted approval reached its durable commit");
    }

    fn resume_approval_commit(&self) {
        self.approval_commit_may_return.notify_one();
    }

    async fn wait_until_approval_request_committed(&self) {
        tokio::time::timeout(
            Duration::from_secs(2),
            self.approval_request_committed.notified(),
        )
        .await
        .expect("the runtime approval request became durable");
    }

    async fn wait_until_renewal_unavailable(&self) {
        tokio::time::timeout(Duration::from_secs(2), self.renewal_unavailable.notified())
            .await
            .expect("the receipted interaction attempted an unavailable renewal");
    }

    async fn gate_receipted_commit(&self, events: &[Event]) {
        if events
            .iter()
            .any(|event| matches!(&event.kind, EventKind::TurnStarted))
            && self.stall_prompt_commit.swap(false, Ordering::SeqCst)
        {
            self.prompt_commit_stalled.notify_one();
            self.prompt_commit_may_return.notified().await;
        }
        if events
            .iter()
            .any(|event| matches!(&event.kind, EventKind::ApprovalResolved { .. }))
            && self.stall_approval_commit.swap(false, Ordering::SeqCst)
        {
            self.approval_commit_stalled.notify_one();
            self.approval_commit_may_return.notified().await;
        }
    }

    fn note_approval_request(&self, events: &[Event], result: &Result<u64, StoreError>) {
        if result.is_ok()
            && events
                .iter()
                .any(|event| matches!(&event.kind, EventKind::ApprovalRequested { .. }))
        {
            self.approval_request_committed.notify_one();
        }
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
        let acquired = self.inner.acquire_interaction_lease(request).await;
        if acquired.is_ok()
            && self
                .stall_acquisition_after_commit
                .swap(false, Ordering::SeqCst)
        {
            self.acquisition_committed.notify_one();
            return std::future::pending().await;
        }
        if acquired.is_ok() && Self::consume(&self.unknown_acquisition_results) {
            return Err(StoreError::Unavailable(
                "interaction acquisition result was lost".into(),
            ));
        }
        acquired
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
        if self.detach_next_renewal.swap(false, Ordering::SeqCst) {
            let inner = Arc::clone(&self.inner);
            let lease = lease.clone();
            let may_commit = Arc::clone(&self.detached_renewal_may_commit);
            let committed = Arc::clone(&self.detached_renewal_committed);
            tokio::spawn(async move {
                may_commit.notified().await;
                inner
                    .renew_interaction_lease(&lease, ttl)
                    .await
                    .expect("the detached test renewal commits");
                committed.store(true, Ordering::SeqCst);
            });
            self.detached_renewal_dispatched
                .store(true, Ordering::SeqCst);
            return std::future::pending().await;
        }
        if Self::consume(&self.renewal_failures) {
            self.renewal_unavailable.notify_one();
            return Err(StoreError::Unavailable(
                "interaction renewal unavailable".into(),
            ));
        }
        let renewed = self.inner.renew_interaction_lease(lease, ttl).await;
        if renewed.is_ok() && Self::consume(&self.unknown_renewal_results) {
            return Err(StoreError::Unavailable(
                "interaction renewal result was lost".into(),
            ));
        }
        renewed
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
        self.gate_receipted_commit(events).await;
        let committed = self
            .inner
            .commit_session_idempotent(write_id, record, events)
            .await;
        self.note_approval_request(events, &committed);
        committed
    }

    async fn commit_conversation_idempotent(
        &self,
        write_id: &str,
        authority: &ConversationWriteAuthority,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError> {
        self.gate_receipted_commit(events).await;
        let committed = self
            .inner
            .commit_conversation_idempotent(write_id, authority, record, events)
            .await;
        self.note_approval_request(events, &committed);
        committed
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

fn versioned_conversation_service(
    store: Arc<InteractionStore>,
    adapter: Arc<FakeAdapter>,
    environments: Arc<crate::environment_service::EnvironmentProfileService>,
    version: &str,
) -> Arc<SessionService> {
    let factory: crate::service::ScreenFactory =
        Arc::new(|geometry| Box::new(NullScreen(geometry)));
    let mut adapters: std::collections::HashMap<KernelId, Arc<dyn KernelAdapter>> =
        std::collections::HashMap::new();
    adapters.insert(kernel(), adapter);
    let service = SessionService::new(
        adapters,
        factory,
        store,
        crate::service::SessionPorts::default(),
        environments,
        crate::service::Deployment {
            runtime_version: Some(version.to_string()),
            ..crate::test_support::owned()
        },
    );
    service.retry_backoff_ms.store(1, Ordering::Relaxed);
    service
}

struct ReceiptRace {
    store: Arc<InteractionStore>,
    adapter: Arc<FakeAdapter>,
    origin: Arc<SessionService>,
    successor: Arc<SessionService>,
    owner: crate::authz::Caller,
    id: SessionId,
}

async fn receipt_race(owner: &str) -> ReceiptRace {
    let store = Arc::new(InteractionStore::default());
    let mut capabilities = conversing();
    capabilities.terminal = true;
    let adapter = Arc::new(FakeAdapter {
        capabilities,
        ..FakeAdapter::default()
    });
    let environments = environments_defaulting_to(AgentConfiguration {
        kernel: kernel(),
        mode: SessionMode::Conversation,
        ..Default::default()
    });
    let origin = versioned_conversation_service(
        Arc::clone(&store),
        Arc::clone(&adapter),
        Arc::clone(&environments),
        "runtime:receipt-origin",
    );
    let successor = versioned_conversation_service(
        Arc::clone(&store),
        Arc::clone(&adapter),
        environments,
        "runtime:receipt-successor",
    );
    let owner = user(owner);
    let created = origin
        .create_session(&owner, spec())
        .await
        .expect("created");
    await_settled(&origin, &owner, &created.id).await;
    ReceiptRace {
        store,
        adapter,
        origin,
        successor,
        owner,
        id: created.id,
    }
}

async fn expire_receipted_interaction(store: &InteractionStore, id: &SessionId) -> SessionLease {
    let held = store
        .inner
        .active_session_lease(id)
        .await
        .expect("the receipted interaction retains its lease");
    assert_eq!(held.operation, SessionOperation::Interact);
    store.fail_renewals(usize::MAX);
    tokio::time::advance(Duration::from_secs(10)).await;
    store.wait_until_renewal_unavailable().await;
    tokio::time::advance(Duration::from_secs(21)).await;
    tokio::task::yield_now().await;
    held
}

async fn start_paused_handover(race: &ReceiptRace) -> tokio::task::JoinHandle<bool> {
    race.adapter.pause_next_provision();
    let durable = race.store.get(&race.id).await.unwrap().unwrap();
    let rotation = tokio::spawn({
        let successor = Arc::clone(&race.successor);
        async move { successor.rotate(durable).await }
    });
    race.adapter.wait_until_provision_paused().await;

    let lease = race
        .store
        .inner
        .active_session_lease(&race.id)
        .await
        .expect("the successor holds durable handover authority");
    assert_eq!(lease.operation, SessionOperation::Handover);
    assert_eq!(
        race.store
            .get(&race.id)
            .await
            .unwrap()
            .unwrap()
            .session
            .status,
        SessionStatus::Rotating
    );
    rotation
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

async fn completed_retained_interaction(
    service: &Arc<SessionService>,
    durable: &SessionRecord,
) -> RetainedRuntimeInteraction {
    let (retained, is_retained) = tokio::sync::oneshot::channel();
    service
        .run_retained_runtime_interaction_at(
            durable,
            move |authority, interaction| async move {
                authority.ensure_delivery_allowed()?;
                let _ = retained.send(interaction);
                Ok::<_, CoreError>(())
            },
            |_reason, _result, _interaction| panic!("an observed result is not undelivered"),
        )
        .await
        .expect("the retained interaction action completes");
    is_retained
        .await
        .expect("the completed action preserves one retained owner")
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
async fn caller_cancellation_recovers_and_releases_an_unknown_acquisition() {
    let store = Arc::new(InteractionStore::default());
    store.inner.seed(&record()).await.unwrap();
    store.stall_next_acquisition_after_commit();
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = store.get(&record().session.id).await.unwrap().unwrap();
    let action_runs = Arc::new(AtomicUsize::new(0));

    let caller = tokio::spawn({
        let service = Arc::clone(&service);
        let action_runs = Arc::clone(&action_runs);
        let durable = durable.clone();
        async move {
            service
                .run_runtime_interaction_at(&durable, move |authority| async move {
                    authority.ensure_delivery_allowed()?;
                    action_runs.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, CoreError>(())
                })
                .await
        }
    });
    tokio::time::timeout(
        Duration::from_secs(2),
        store.acquisition_committed.notified(),
    )
    .await
    .expect("the first acquisition commits before its response stalls");
    let hidden = store
        .inner
        .active_session_lease(&durable.session.id)
        .await
        .expect("the stalled response hides an active interaction lease");

    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    wait_for_releases(&store, 1).await;
    assert_eq!(action_runs.load(Ordering::SeqCst), 0);
    assert!(store
        .inner
        .active_session_lease(&durable.session.id)
        .await
        .is_none());

    let acquisitions = store.acquisitions.lock().await;
    assert_eq!(acquisitions.len(), 2);
    assert_eq!(acquisitions[0].nonce, acquisitions[1].nonce);
    drop(acquisitions);
    assert_eq!(store.releases.lock().await.as_slice(), &[hidden.clone()]);

    service
        .run_runtime_interaction_at(&durable, |authority| async move {
            authority.ensure_delivery_allowed()?;
            Ok::<_, CoreError>(())
        })
        .await
        .expect("a successor acquires before the hidden lease TTL");
    wait_for_releases(&store, 2).await;
    let releases = store.releases.lock().await;
    assert_ne!(releases[0].token, releases[1].token);
}

#[tokio::test(start_paused = true)]
async fn unknown_acquisition_recovery_outlives_every_hidden_lease_extension() {
    let store = Arc::new(InteractionStore::default());
    store.inner.seed(&record()).await.unwrap();
    // Initial acquisition plus recovery at t=0 and once per second through
    // t=29 all commit successfully while hiding their same-nonce response.
    store.obscure_acquisition_successes(31);
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = store.get(&record().session.id).await.unwrap().unwrap();
    let action_runs = Arc::new(AtomicUsize::new(0));

    let result = service
        .run_runtime_interaction_at(&durable, {
            let action_runs = Arc::clone(&action_runs);
            move |authority| async move {
                authority.ensure_delivery_allowed()?;
                action_runs.fetch_add(1, Ordering::SeqCst);
                Ok::<_, CoreError>(())
            }
        })
        .await;
    assert!(matches!(result, Err(CoreError::RuntimeUnavailable(_))));
    while store.acquisitions.lock().await.len() < 2 {
        tokio::task::yield_now().await;
    }
    for expected in 3..=31 {
        tokio::time::advance(Duration::from_secs(1)).await;
        while store.acquisitions.lock().await.len() < expected {
            tokio::task::yield_now().await;
        }
    }
    assert_eq!(action_runs.load(Ordering::SeqCst), 0);
    assert!(store
        .inner
        .active_session_lease(&durable.session.id)
        .await
        .is_some());

    // At t=30 the definitive same-nonce response resolves every earlier
    // overlapping attempt and cleanup switches irreversibly to exact release.
    tokio::time::advance(Duration::from_secs(1)).await;
    wait_for_releases(&store, 1).await;
    assert!(store
        .inner
        .active_session_lease(&durable.session.id)
        .await
        .is_none());
    let acquisitions = store.acquisitions.lock().await;
    assert_eq!(acquisitions.len(), 32);
    assert!(acquisitions
        .windows(2)
        .all(|attempts| attempts[0].nonce == attempts[1].nonce));
    drop(acquisitions);

    service
        .run_runtime_interaction_at(&durable, |authority| async move {
            authority.ensure_delivery_allowed()?;
            Ok::<_, CoreError>(())
        })
        .await
        .expect("a successor acquires before the t=59 hidden lease expiry");
    wait_for_releases(&store, 2).await;
    let releases = store.releases.lock().await;
    assert_ne!(releases[0].token, releases[1].token);
}

#[test]
fn unknown_acquisition_cleanup_survives_origin_runtime_destruction() {
    let origin = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("origin runtime builds");
    let store = Arc::new(InteractionStore::default());
    origin.block_on(store.inner.seed(&record())).unwrap();
    store.stall_next_acquisition_after_commit();
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = origin
        .block_on(store.get(&record().session.id))
        .unwrap()
        .unwrap();
    let action_runs = Arc::new(AtomicUsize::new(0));

    let caller = origin.block_on(async {
        let caller = tokio::spawn({
            let service = Arc::clone(&service);
            let durable = durable.clone();
            let action_runs = Arc::clone(&action_runs);
            async move {
                service
                    .run_runtime_interaction_at(&durable, move |authority| async move {
                        authority.ensure_delivery_allowed()?;
                        action_runs.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, CoreError>(())
                    })
                    .await
            }
        });
        tokio::time::timeout(
            Duration::from_secs(2),
            store.acquisition_committed.notified(),
        )
        .await
        .expect("the origin polls the acquisition through its hidden commit");
        caller
    });
    let hidden = origin
        .block_on(store.inner.active_session_lease(&durable.session.id))
        .expect("the origin runtime owns a hidden exact lease");
    drop(origin);
    drop(caller);

    let replacement = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("replacement runtime builds");
    replacement.block_on(async {
        service
            .run_runtime_interaction_at(&durable, |authority| async move {
                authority.ensure_delivery_allowed()?;
                Ok::<_, CoreError>(())
            })
            .await
            .expect("a successor drains origin-owned acquisition recovery");
        wait_for_releases(&store, 2).await;
    });

    assert_eq!(action_runs.load(Ordering::SeqCst), 0);
    let acquisitions = replacement.block_on(store.acquisitions.lock());
    assert!(
        acquisitions
            .iter()
            .filter(|request| request.nonce == acquisitions[0].nonce)
            .count()
            >= 2
    );
    drop(acquisitions);
    let releases = replacement.block_on(store.releases.lock());
    assert_eq!(releases[0], hidden);
    assert_ne!(releases[0].token, releases[1].token);
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

#[tokio::test]
async fn final_retained_owner_invalidates_detached_authority_before_a_successor() {
    let store = Arc::new(InteractionStore::default());
    store.inner.seed(&record()).await.unwrap();
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = store.get(&record().session.id).await.unwrap().unwrap();

    let interaction = service
        .run_retained_runtime_interaction_at(
            &durable,
            |authority, interaction| async move {
                authority.ensure_delivery_allowed()?;
                Ok::<_, CoreError>(interaction)
            },
            |_reason, _result, _interaction| panic!("an observed result is not undelivered"),
        )
        .await
        .expect("the retained owner is delivered");
    let stale_authority = interaction.authority();
    stale_authority
        .ensure_delivery_allowed()
        .expect("the final retained owner still carries delivery authority");

    drop(interaction);
    assert!(
        matches!(
            stale_authority.ensure_delivery_allowed(),
            Err(CoreError::RuntimeUnavailable(_))
        ),
        "final-owner drop invalidates detached authority synchronously"
    );

    service
        .run_runtime_interaction_at(&durable, |authority| async move {
            authority.ensure_delivery_allowed()?;
            Ok::<_, CoreError>(())
        })
        .await
        .expect("the exact release lets a successor acquire");
    assert!(matches!(
        stale_authority.ensure_delivery_allowed(),
        Err(CoreError::RuntimeUnavailable(_))
    ));
    wait_for_releases(&store, 2).await;
    let releases = store.releases.lock().await;
    assert_ne!(releases[0].token, releases[1].token);
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
async fn retained_resume_retries_transient_renewal_until_the_store_recovers() {
    let store = Arc::new(InteractionStore::default());
    store.inner.seed(&record()).await.unwrap();
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = store.get(&record().session.id).await.unwrap().unwrap();
    let interaction = completed_retained_interaction(&service, &durable).await;
    store.fail_renewals(2);

    let resumed = tokio::spawn({
        let interaction = interaction.clone();
        async move { interaction.resume_before_delivery().await }
    });
    while store.renewals.lock().await.is_empty() {
        tokio::task::yield_now().await;
    }
    for expected_attempts in 2..=3 {
        tokio::time::advance(Duration::from_secs(1)).await;
        while store.renewals.lock().await.len() < expected_attempts {
            tokio::task::yield_now().await;
        }
    }

    resumed
        .await
        .unwrap()
        .expect("the exact retained lease renews after recovery")
        .ensure_delivery_allowed()
        .expect("renewal restores a full delivery window");
    let renewals = store.renewals.lock().await;
    assert_eq!(renewals.len(), 3);
    assert_eq!(renewals[0].0, renewals[1].0);
    assert_eq!(renewals[1].0, renewals[2].0);
    drop(renewals);

    drop(interaction);
    wait_for_releases(&store, 1).await;
}

#[tokio::test(start_paused = true)]
async fn unknown_retained_renewals_extend_exact_cleanup_until_release_recovers() {
    let store = Arc::new(InteractionStore::default());
    store.inner.seed(&record()).await.unwrap();
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = store.get(&record().session.id).await.unwrap().unwrap();
    let interaction = completed_retained_interaction(&service, &durable).await;

    tokio::time::advance(Duration::from_secs(20)).await;
    store.obscure_renewal_successes(2);
    let resumed = tokio::spawn({
        let interaction = interaction.clone();
        async move { interaction.resume_before_delivery().await }
    });
    while store.renewals.lock().await.is_empty() {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_secs(1)).await;
    while store.unknown_renewal_results.load(Ordering::SeqCst) != 0 {
        tokio::task::yield_now().await;
    }
    assert_eq!(store.renewals.lock().await.len(), 2);
    resumed.abort();
    assert!(matches!(
        resumed.await,
        Err(error) if error.is_cancelled()
    ));

    // Keep exact release unavailable through the original t=30 bound. Both
    // hidden-success renewals extended the store lease beyond it, so cleanup
    // must keep retrying the same token instead of assuming it expired.
    store.fail_releases(usize::MAX);
    drop(interaction);
    while store.releases.lock().await.is_empty() {
        tokio::task::yield_now().await;
    }
    for expected_attempts in 2..=9 {
        tokio::time::advance(Duration::from_secs(1)).await;
        while store.releases.lock().await.len() < expected_attempts {
            tokio::task::yield_now().await;
        }
    }
    assert!(store
        .inner
        .active_session_lease(&durable.session.id)
        .await
        .is_some());

    store.fail_releases(0);
    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while store.releases.lock().await.len() < 10 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cleanup retries beyond the pre-renewal expiry bound");
    assert!(store
        .inner
        .active_session_lease(&durable.session.id)
        .await
        .is_none());

    service
        .run_runtime_interaction_at(&durable, |authority| async move {
            authority.ensure_delivery_allowed()?;
            Ok::<_, CoreError>(())
        })
        .await
        .expect("exact cleanup lets a successor acquire promptly");
    wait_for_releases(&store, 11).await;
    let releases = store.releases.lock().await;
    assert!(releases[..10]
        .windows(2)
        .all(|attempts| attempts[0] == attempts[1]));
    assert_ne!(releases[9].token, releases[10].token);
}

#[tokio::test(start_paused = true)]
async fn canceled_in_flight_renewal_keeps_exact_cleanup_until_hidden_commit_releases() {
    let store = Arc::new(InteractionStore::default());
    store.inner.seed(&record()).await.unwrap();
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = store.get(&record().session.id).await.unwrap().unwrap();
    let interaction = completed_retained_interaction(&service, &durable).await;

    tokio::time::advance(Duration::from_secs(20)).await;
    store.detach_next_renewal();
    let resumed = tokio::spawn({
        let interaction = interaction.clone();
        async move { interaction.resume_before_delivery().await }
    });
    while !store.detached_renewal_dispatched.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }

    resumed.abort();
    assert!(matches!(
        resumed.await,
        Err(error) if error.is_cancelled()
    ));
    store.fail_releases(usize::MAX);
    drop(interaction);
    wait_for_releases(&store, 1).await;

    // The store commits at t=21 after the only waiter was canceled, extending
    // its exact lease to t=51 without returning an outcome to the service.
    tokio::time::advance(Duration::from_secs(1)).await;
    store.allow_detached_renewal_to_commit();
    while !store.detached_renewal_committed.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }

    // Drive failed exact releases through t=49. A finite drop-time-plus-TTL
    // bound would stop cleanup at t=50 even though the hidden renewal is live.
    let mut attempts = store.releases.lock().await.len();
    for _ in 22..50 {
        tokio::time::advance(Duration::from_secs(1)).await;
        while store.releases.lock().await.len() <= attempts {
            tokio::task::yield_now().await;
        }
        attempts = store.releases.lock().await.len();
    }
    assert!(store
        .inner
        .active_session_lease(&durable.session.id)
        .await
        .is_some());

    store.fail_releases(0);
    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::time::timeout(Duration::from_secs(1), async {
        while store
            .inner
            .active_session_lease(&durable.session.id)
            .await
            .is_some()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("unknown-expiry cleanup releases the still-live exact lease at t=50");

    let cleanup_attempts = store.releases.lock().await.len();
    service
        .run_runtime_interaction_at(&durable, |authority| async move {
            authority.ensure_delivery_allowed()?;
            Ok::<_, CoreError>(())
        })
        .await
        .expect("exact cleanup lets a successor acquire before hidden expiry");
    wait_for_releases(&store, cleanup_attempts + 1).await;
    let releases = store.releases.lock().await;
    assert!(releases[..cleanup_attempts]
        .windows(2)
        .all(|attempts| attempts[0] == attempts[1]));
    assert_ne!(
        releases[cleanup_attempts - 1].token,
        releases[cleanup_attempts].token
    );
}

#[tokio::test]
async fn retained_resume_rejects_a_confirmed_exact_lease_replacement() {
    let store = Arc::new(InteractionStore::default());
    store.inner.seed(&record()).await.unwrap();
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = store.get(&record().session.id).await.unwrap().unwrap();
    let interaction = completed_retained_interaction(&service, &durable).await;
    let old_lease = store
        .inner
        .active_session_lease(&durable.session.id)
        .await
        .expect("the retained owner keeps its exact lease");
    store.inner.release_session_lease(&old_lease).await.unwrap();
    let successor_lease = service
        .acquire_runtime_lease_at(&durable, SessionOperation::Interact)
        .await
        .unwrap()
        .expect("a replacement interaction lease is acquired");
    assert_ne!(old_lease.token, successor_lease.token);

    assert!(matches!(
        interaction.resume_before_delivery().await,
        Err(CoreError::RuntimeUnavailable(_))
    ));
    assert!(matches!(
        interaction.authority().ensure_delivery_allowed(),
        Err(CoreError::RuntimeUnavailable(_))
    ));
    assert_eq!(store.renewals.lock().await.len(), 1);

    drop(interaction);
    wait_for_releases(&store, 1).await;
    assert_eq!(
        store.inner.active_session_lease(&durable.session.id).await,
        Some(successor_lease.clone()),
        "cleanup of the confirmed old lease cannot clear its replacement"
    );
    store
        .inner
        .release_session_lease(&successor_lease)
        .await
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn retained_resume_refuses_an_expired_delivery_without_renewing() {
    let store = Arc::new(InteractionStore::default());
    store.inner.seed(&record()).await.unwrap();
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = store.get(&record().session.id).await.unwrap().unwrap();
    let interaction = completed_retained_interaction(&service, &durable).await;

    tokio::time::advance(Duration::from_secs(30)).await;
    assert!(matches!(
        interaction.resume_before_delivery().await,
        Err(CoreError::RuntimeUnavailable(_))
    ));
    assert!(store.renewals.lock().await.is_empty());
    drop(interaction);

    service
        .run_runtime_interaction_at(&durable, |authority| async move {
            authority.ensure_delivery_allowed()?;
            Ok::<_, CoreError>(())
        })
        .await
        .expect("the store expires the old lease before admitting a successor");
}

#[tokio::test]
async fn retained_resume_observes_caller_invalidation_before_renewing() {
    let store = Arc::new(InteractionStore::default());
    store.inner.seed(&record()).await.unwrap();
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = store.get(&record().session.id).await.unwrap().unwrap();
    let (retained, is_retained) = tokio::sync::oneshot::channel();
    let (finish, may_finish) = tokio::sync::oneshot::channel();
    let caller = tokio::spawn({
        let service = Arc::clone(&service);
        async move {
            service
                .run_retained_runtime_interaction_at(
                    &durable,
                    move |authority, interaction| async move {
                        authority.ensure_delivery_allowed()?;
                        let _ = retained.send(interaction);
                        let _ = may_finish.await;
                        Ok::<_, CoreError>(())
                    },
                    |_reason, _result, _interaction| {},
                )
                .await
        }
    });
    let interaction = is_retained
        .await
        .expect("the running action preserves one retained owner");

    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert!(matches!(
        interaction.resume_before_delivery().await,
        Err(CoreError::RuntimeUnavailable(_))
    ));
    assert!(store.renewals.lock().await.is_empty());

    finish.send(()).unwrap();
    drop(interaction);
    wait_for_releases(&store, 1).await;
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

    // The retained continuation runs on a runtime that is deliberately
    // independent of this test's paused clock. Resume real time now that the
    // renewal horizon assertions are complete so the cleanup deadline cannot
    // auto-elapse before that executor receives a scheduling turn.
    tokio::time::resume();
    store.resume_prompt_commit();
    store.wait_until_interaction_released().await;
    assert!(store
        .inner
        .active_session_lease(&created.id)
        .await
        .is_none());
    assert_eq!(
        store.releases.lock().await.as_slice(),
        &[held],
        "prompt finalization releases its exact interaction lease once"
    );
    assert_eq!(
        store
            .get(&created.id)
            .await
            .unwrap()
            .unwrap()
            .session
            .status,
        SessionStatus::Running,
        "the optional running transition settles before interaction cleanup"
    );
    let events = service
        .events_since(&alice, &created.id, None)
        .await
        .expect("the finalized prompt history is readable");
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        EventKind::OwnerMessage { content } if content == "retain the finalization fence"
    )));
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    EventKind::StatusChanged {
                        status: SessionStatus::Running,
                        ..
                    }
                )
            })
            .count(),
        1,
        "one receipted idle prompt records one running transition"
    );

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

#[tokio::test(start_paused = true)]
async fn a_fenced_running_proposal_retires_its_superseded_conversation_link() {
    let race = receipt_race("running-fence-readoption-owner").await;
    let (old_updates, old_received) = {
        let conversation = race.adapter.conversation.lock().await;
        let conversation = conversation
            .as_ref()
            .expect("the origin conversation is live");
        (
            conversation.updates.clone(),
            Arc::clone(&conversation.received),
        )
    };
    let (old_link, old_authority) = {
        let entries = race.origin.entries.read().await;
        let entry = entries
            .get(&race.id)
            .expect("the origin entry is installed");
        (
            entry.conversation_link.expect("the origin link is named"),
            entry
                .conversation_authority
                .clone()
                .expect("the origin link has durable authority"),
        )
    };

    race.store.stall_next_prompt_commit();
    let prompt = tokio::spawn({
        let service = Arc::clone(&race.origin);
        let owner = race.owner.clone();
        let id = race.id.clone();
        async move {
            service
                .send_prompt(&owner, &id, "receipt before readoption".into())
                .await
        }
    });
    race.store.wait_until_prompt_commit_stalled().await;
    let _expired = expire_receipted_interaction(&race.store, &race.id).await;

    let durable = race
        .store
        .get(&race.id)
        .await
        .unwrap()
        .expect("the same runtime remains durable");
    race.successor
        .readopt(&durable)
        .await
        .expect("a successor republishes the same runtime");
    let replacement_authority = race
        .successor
        .entries
        .read()
        .await
        .get(&race.id)
        .expect("the replacement conversation is installed")
        .conversation_authority
        .clone()
        .expect("the replacement link has durable authority");
    assert_eq!(
        replacement_authority.runtime_generation,
        old_authority.runtime_generation
    );
    assert_eq!(replacement_authority.runtime_id, old_authority.runtime_id);
    assert_ne!(replacement_authority.epoch, old_authority.epoch);
    assert!(
        race.origin
            .entries
            .read()
            .await
            .get(&race.id)
            .is_some_and(|entry| {
                entry.conversation_link == Some(old_link)
                    && entry.conversation_authority.as_ref() == Some(&old_authority)
            }),
        "the still-open predecessor remains installed until its fenced proposal settles"
    );
    assert!(!old_updates.is_closed());

    tokio::time::resume();
    race.store.resume_prompt_commit();
    prompt
        .await
        .expect("the origin prompt task completes")
        .expect("definite prompt history survives same-runtime readoption");
    assert!(
        !race.origin.entries.read().await.contains_key(&race.id),
        "proposal rejection retires the exact superseded local link"
    );
    assert!(matches!(
        old_received.lock().await.as_slice(),
        [ConversationInput::Prompt(_)]
    ));

    race.origin
        .send_prompt(&race.owner, &race.id, "use a fresh link".into())
        .await
        .expect("the origin readopts instead of reusing its stale sender");
    let fresh = race
        .adapter
        .conversation
        .lock()
        .await
        .as_ref()
        .expect("a fresh conversation link is live")
        .sent()
        .await;
    assert!(matches!(fresh.as_slice(), [ConversationInput::Prompt(_)]));
    assert!(matches!(
        old_received.lock().await.as_slice(),
        [ConversationInput::Prompt(_)]
    ));
    drop(old_updates);
}

#[tokio::test(start_paused = true)]
async fn receipted_prompt_history_rebases_over_a_successor_handover() {
    let race = receipt_race("prompt-receipt-handover-owner").await;
    race.store.stall_next_prompt_commit();
    let prompt = tokio::spawn({
        let service = Arc::clone(&race.origin);
        let owner = race.owner.clone();
        let id = race.id.clone();
        async move {
            service
                .send_prompt(&owner, &id, "history survives handover".into())
                .await
        }
    });
    race.store.wait_until_prompt_commit_stalled().await;
    let expired = expire_receipted_interaction(&race.store, &race.id).await;

    let rotation = start_paused_handover(&race).await;
    let handover = race
        .store
        .inner
        .active_session_lease(&race.id)
        .await
        .expect("the successor replaced the expired interaction lease");
    assert_ne!(handover.token, expired.token);
    let sent = race
        .adapter
        .conversation
        .lock()
        .await
        .as_ref()
        .expect("the outgoing conversation remains live until replacement")
        .sent()
        .await;
    assert!(matches!(sent.as_slice(), [ConversationInput::Prompt(_)]));

    race.store.resume_prompt_commit();
    let cursor = prompt
        .await
        .expect("the origin prompt task completes")
        .expect("the receipted prompt history rebases over Rotating");
    let events = race
        .origin
        .events_since(&race.owner, &race.id, None)
        .await
        .expect("the rebased prompt history is readable");
    let rotating_seq = events
        .iter()
        .find(|event| {
            matches!(
                event.kind,
                EventKind::StatusChanged {
                    status: SessionStatus::Rotating,
                    ..
                }
            )
        })
        .expect("the successor durably announced its handover")
        .seq;
    let turns = events
        .iter()
        .filter(|event| matches!(event.kind, EventKind::TurnStarted))
        .collect::<Vec<_>>();
    let messages = events
        .iter()
        .filter(|event| {
            matches!(
                &event.kind,
                EventKind::OwnerMessage { content } if content == "history survives handover"
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(turns.len(), 1, "one receipt records one turn boundary");
    assert_eq!(messages.len(), 1, "one receipt records one owner message");
    assert_eq!(cursor, turns[0].seq);
    assert_eq!(messages[0].seq, turns[0].seq + 1);
    assert!(turns[0].seq > rotating_seq);
    assert!(
        events.iter().all(|event| {
            event.seq <= rotating_seq
                || !matches!(
                    event.kind,
                    EventKind::StatusChanged {
                        status: SessionStatus::Running,
                        ..
                    }
                )
        }),
        "the origin must not append a stale Running transition after Rotating: {events:?}"
    );
    assert_eq!(
        race.store
            .get(&race.id)
            .await
            .unwrap()
            .unwrap()
            .session
            .status,
        SessionStatus::Rotating,
        "history-only finalization cannot settle the successor's handover"
    );
    assert_eq!(
        race.store
            .inner
            .active_session_lease(&race.id)
            .await
            .expect("the successor still owns its handover")
            .operation,
        SessionOperation::Handover
    );

    race.adapter.resume_provision();
    assert!(rotation.await.expect("the handover task completes"));
    let settled = await_settled(&race.successor, &race.owner, &race.id).await;
    assert_ne!(settled.status, SessionStatus::Rotating);
    assert!(
        race.store
            .runtime_authority(&race.id)
            .await
            .unwrap()
            .expect("the replacement authority is durable")
            .generation
            > 0
    );
}

#[tokio::test(start_paused = true)]
async fn receipted_approval_history_rebases_over_a_successor_handover() {
    const REQUEST_ID: &str = "approval-receipt-before-handover";

    let race = receipt_race("approval-receipt-handover-owner").await;
    let updates = race
        .adapter
        .conversation
        .lock()
        .await
        .as_ref()
        .expect("the conversation is open")
        .updates
        .clone();
    updates
        .send(layer3_adapter::ConversationUpdate::Asked(asking(
            REQUEST_ID,
            "preserve one approval receipt",
        )))
        .await
        .expect("the core is listening");
    race.store.wait_until_approval_request_committed().await;
    assert_eq!(
        race.store
            .get(&race.id)
            .await
            .unwrap()
            .unwrap()
            .session
            .status,
        SessionStatus::Idle
    );
    assert_eq!(
        race.origin
            .entries
            .read()
            .await
            .get(&race.id)
            .expect("the origin conversation remains installed")
            .pending_approvals
            .get(REQUEST_ID),
        Some(&None)
    );

    race.store.stall_next_approval_commit();
    let approval = tokio::spawn({
        let service = Arc::clone(&race.origin);
        let owner = race.owner.clone();
        let id = race.id.clone();
        async move {
            service
                .resolve_approval(&owner, &id, REQUEST_ID.into(), ApprovalDecision::Allow)
                .await
        }
    });
    race.store.wait_until_approval_commit_stalled().await;
    assert_eq!(
        race.origin
            .entries
            .read()
            .await
            .get(&race.id)
            .expect("the receipted approval remains locally owned")
            .pending_approvals
            .get(REQUEST_ID),
        Some(&None),
        "a receipt alone cannot remove pending state before ApprovalResolved is durable"
    );
    let expired = expire_receipted_interaction(&race.store, &race.id).await;

    let rotation = start_paused_handover(&race).await;
    let handover = race
        .store
        .inner
        .active_session_lease(&race.id)
        .await
        .expect("the successor replaced the expired interaction lease");
    assert_ne!(handover.token, expired.token);
    let sent = race
        .adapter
        .conversation
        .lock()
        .await
        .as_ref()
        .expect("the outgoing conversation remains live until replacement")
        .sent()
        .await;
    assert!(matches!(
        sent.as_slice(),
        [ConversationInput::PermissionAnswer { id, .. }] if id == REQUEST_ID
    ));
    assert_eq!(
        race.origin
            .entries
            .read()
            .await
            .get(&race.id)
            .expect("the origin entry remains installed")
            .pending_approvals
            .get(REQUEST_ID),
        Some(&None)
    );

    race.store.resume_approval_commit();
    approval
        .await
        .expect("the origin approval task completes")
        .expect("the receipted approval history rebases over Rotating");
    let events = race
        .origin
        .events_since(&race.owner, &race.id, None)
        .await
        .expect("the rebased approval history is readable");
    let rotating_seq = events
        .iter()
        .find(|event| {
            matches!(
                event.kind,
                EventKind::StatusChanged {
                    status: SessionStatus::Rotating,
                    ..
                }
            )
        })
        .expect("the successor durably announced its handover")
        .seq;
    let resolved = events
        .iter()
        .filter(|event| {
            matches!(
                &event.kind,
                EventKind::ApprovalResolved { request_id, allowed }
                    if request_id == REQUEST_ID && *allowed
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(resolved.len(), 1, "one receipt records one resolved prefix");
    assert!(resolved[0].seq > rotating_seq);
    assert!(
        events.iter().all(|event| {
            event.seq <= rotating_seq
                || !matches!(
                    event.kind,
                    EventKind::StatusChanged {
                        status: SessionStatus::Running,
                        ..
                    }
                )
        }),
        "the origin must not append a stale Running transition after Rotating: {events:?}"
    );
    let approval_state = crate::conversation_log::pending_approvals_from(&events)
        .expect("the durable approval history folds");
    assert!(approval_state.pending.is_empty());
    assert!(
        !race
            .origin
            .entries
            .read()
            .await
            .get(&race.id)
            .expect("the origin entry remains installed")
            .pending_approvals
            .contains_key(REQUEST_ID),
        "only durable ApprovalResolved removes the private pending entry"
    );
    assert_eq!(
        race.store
            .get(&race.id)
            .await
            .unwrap()
            .unwrap()
            .session
            .status,
        SessionStatus::Rotating,
        "approval history finalization cannot settle the successor's handover"
    );
    assert_eq!(
        race.store
            .inner
            .active_session_lease(&race.id)
            .await
            .expect("the successor still owns its handover")
            .operation,
        SessionOperation::Handover
    );

    race.adapter.resume_provision();
    assert!(rotation.await.expect("the handover task completes"));
    let settled = await_settled(&race.successor, &race.owner, &race.id).await;
    assert_ne!(settled.status, SessionStatus::Rotating);
    let successor_entries = race.successor.entries.read().await;
    assert!(
        successor_entries
            .get(&race.id)
            .expect("the replacement conversation is installed")
            .pending_approvals
            .is_empty(),
        "the successor rebuilds no pending request after durable resolution"
    );
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

#[tokio::test(start_paused = true)]
async fn transient_release_outage_retains_cleanup_until_a_successor_can_proceed() {
    let store = Arc::new(InteractionStore::default());
    store.inner.seed(&record()).await.unwrap();
    store.fail_releases(3);
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = store.get(&record().session.id).await.unwrap().unwrap();

    let first = service
        .run_runtime_interaction_at(&durable, |authority| async move {
            authority.ensure_delivery_allowed()?;
            Ok::<_, CoreError>("known-success")
        })
        .await
        .expect("the known result precedes release recovery");
    assert_eq!(first, "known-success");
    while store.releases.lock().await.is_empty() {
        tokio::task::yield_now().await;
    }

    let successor = tokio::spawn({
        let service = Arc::clone(&service);
        let durable = durable.clone();
        async move {
            service
                .run_runtime_interaction_at(&durable, |authority| async move {
                    authority.ensure_delivery_allowed()?;
                    Ok::<_, CoreError>("successor")
                })
                .await
        }
    });

    for expected_attempts in 2..=3 {
        tokio::time::advance(Duration::from_secs(1)).await;
        while store.releases.lock().await.len() < expected_attempts {
            tokio::task::yield_now().await;
        }
        assert!(
            !successor.is_finished(),
            "the exact old lease remains fenced while release is unavailable"
        );
    }
    tokio::time::advance(Duration::from_secs(1)).await;

    assert_eq!(successor.await.unwrap().unwrap(), "successor");
    wait_for_releases(&store, 5).await;
    let releases = store.releases.lock().await;
    assert_eq!(releases[0], releases[1]);
    assert_eq!(releases[1], releases[2]);
    assert_eq!(releases[2], releases[3]);
    assert_ne!(releases[3].token, releases[4].token);
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

#[test]
fn release_interrupted_after_commit_restarts_without_releasing_a_successor() {
    let origin = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("origin runtime builds");
    let store = Arc::new(InteractionStore::default());
    origin.block_on(store.inner.seed(&record())).unwrap();
    store.stall_next_release_after_commit();
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let durable = origin
        .block_on(store.get(&record().session.id))
        .unwrap()
        .unwrap();

    let first = origin
        .block_on(
            service.run_runtime_interaction_at(&durable, |authority| async move {
                authority.ensure_delivery_allowed()?;
                Ok::<_, CoreError>("first")
            }),
        )
        .expect("the action result is known before release responds");
    assert_eq!(first, "first");
    origin.block_on(async {
        tokio::time::timeout(Duration::from_secs(2), store.release_committed.notified())
            .await
            .expect("the exact release commits before its response is lost");
    });
    let old_lease = origin.block_on(async {
        store
            .releases
            .lock()
            .await
            .first()
            .cloned()
            .expect("the old exact release was attempted")
    });
    assert!(origin
        .block_on(store.inner.active_session_lease(&durable.session.id))
        .is_none());

    drop(origin);

    let replacement = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("replacement runtime builds");
    replacement.block_on(async {
        let (started, is_started) = tokio::sync::oneshot::channel();
        let (finish, may_finish) = tokio::sync::oneshot::channel();
        let successor = tokio::spawn({
            let service = Arc::clone(&service);
            let durable = durable.clone();
            async move {
                service
                    .run_runtime_interaction_at(&durable, move |authority| async move {
                        authority.ensure_delivery_allowed()?;
                        let _ = started.send(());
                        let _ = may_finish.await;
                        Ok::<_, CoreError>(())
                    })
                    .await
            }
        });
        is_started
            .await
            .expect("a successor acquires after the old release committed");
        let successor_lease = store
            .inner
            .active_session_lease(&durable.session.id)
            .await
            .expect("the successor holds its own exact lease");
        assert_ne!(old_lease.token, successor_lease.token);

        service
            .event_logs
            .drain_retained_cleanups(&durable.session.id)
            .await;
        assert_eq!(
            store.inner.active_session_lease(&durable.session.id).await,
            Some(successor_lease.clone()),
            "restarted old cleanup cannot clear a successor lease"
        );

        finish.send(()).unwrap();
        successor.await.unwrap().unwrap();
        wait_for_releases(&store, 3).await;
        let releases = store.releases.lock().await;
        assert_eq!(releases[0], old_lease);
        assert_eq!(releases[1], old_lease);
        assert_eq!(releases[2], successor_lease);
    });
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
