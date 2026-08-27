//! Live-session limit behavior at the asking boundary.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use layer3_adapter::KernelAdapter;

use crate::authz::{Authorization, Caller, Capability};
use crate::event::{Event, StoredEvent};
use crate::ids::{KernelId, SessionId, SubjectId};
use crate::service::{ScreenFactory, SessionPorts, SessionService};
use crate::session::SessionStatus;
use crate::store::{
    PlatformSettingsStore, SessionRecord, SessionStore, StoreError, StoredSettings,
};
use crate::test_support::{
    await_settled, no_environments, owned, spec, user, FakeAdapter, FakeStore, NullScreen,
};
use crate::{CoreError, PlatformSettings, PlatformSettingsService};

#[derive(Clone, Copy)]
enum CountMode {
    Delegate,
    Fixed(u32),
    Fail,
}

/// A store that makes count queries and the count-to-write race observable.
struct ObservedStore {
    inner: Arc<FakeStore>,
    mode: CountMode,
    slow_writes: bool,
    count_calls: AtomicUsize,
}

impl ObservedStore {
    fn new(inner: Arc<FakeStore>, mode: CountMode) -> Self {
        inner.disable_coordination();
        Self {
            inner,
            mode,
            slow_writes: false,
            count_calls: AtomicUsize::new(0),
        }
    }

    fn racing(inner: Arc<FakeStore>) -> Self {
        let mut store = Self::new(inner, CountMode::Delegate);
        store.slow_writes = true;
        store
    }

    fn calls(&self) -> usize {
        self.count_calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl SessionStore for ObservedStore {
    async fn ping(&self) -> Result<(), StoreError> {
        self.inner.ping().await
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
        self.inner.get(id).await
    }

    async fn list_for(
        &self,
        owner: &SubjectId,
        query: &crate::SessionListQuery,
    ) -> Result<crate::Page<SessionRecord>, StoreError> {
        self.inner.list_for(owner, query).await
    }

    async fn count_live_for(&self, owner: &SubjectId) -> Result<u32, StoreError> {
        self.count_calls.fetch_add(1, Ordering::SeqCst);
        match self.mode {
            CountMode::Delegate => self.inner.count_live_for(owner).await,
            CountMode::Fixed(count) => Ok(count),
            CountMode::Fail => Err(StoreError::Unavailable(
                "the live count must not be queried".into(),
            )),
        }
    }

    async fn reserved_capacity(&self) -> Result<crate::Reserved, StoreError> {
        self.inner.reserved_capacity().await
    }

    async fn list_all(
        &self,
        query: &crate::SessionListQuery,
    ) -> Result<crate::Page<SessionRecord>, StoreError> {
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
        if self.slow_writes {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
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

struct FixedSettings {
    settings: PlatformSettings,
    broken: bool,
}

#[async_trait::async_trait]
impl PlatformSettingsStore for FixedSettings {
    async fn load(&self) -> Result<Option<StoredSettings>, StoreError> {
        if self.broken {
            return Err(StoreError::Unavailable("settings cannot be read".into()));
        }
        Ok(Some(StoredSettings {
            settings: self.settings.clone(),
            revision: 1,
            previous: None,
        }))
    }

    async fn save_if_unchanged(
        &self,
        _expected: u64,
        _settings: &PlatformSettings,
    ) -> Result<Option<StoredSettings>, StoreError> {
        Err(StoreError::Unavailable(
            "this fixture exposes fixed settings".into(),
        ))
    }
}

fn settings(limit: Option<u32>) -> Arc<PlatformSettingsService> {
    let settings = PlatformSettings {
        live_sessions: limit.map(|value| NonZeroU32::new(value).expect("non-zero limit")),
        ..PlatformSettings::default()
    };
    Arc::new(PlatformSettingsService::new(Arc::new(FixedSettings {
        settings,
        broken: false,
    })))
}

fn broken_settings() -> Arc<PlatformSettingsService> {
    Arc::new(PlatformSettingsService::new(Arc::new(FixedSettings {
        settings: PlatformSettings::default(),
        broken: true,
    })))
}

fn service_with_settings(
    store: Arc<dyn SessionStore>,
    adapter: Arc<FakeAdapter>,
    settings: Arc<PlatformSettingsService>,
) -> Arc<SessionService> {
    let factory: ScreenFactory = Arc::new(|geometry| Box::new(NullScreen(geometry)));
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(crate::test_support::kernel(), adapter);
    let service = SessionService::new(
        adapters,
        factory,
        store,
        SessionPorts {
            settings: Some(settings),
            ..Default::default()
        },
        no_environments(),
        owned(),
    );
    service
        .retry_backoff_ms
        .store(1, std::sync::atomic::Ordering::Relaxed);
    service
}

fn limited(
    store: Arc<dyn SessionStore>,
    adapter: Arc<FakeAdapter>,
    limit: u32,
) -> Arc<SessionService> {
    service_with_settings(store, adapter, settings(Some(limit)))
}

#[tokio::test]
async fn creation_at_the_limit_is_refused_before_any_record_exists() {
    let inner = Arc::new(FakeStore::default());
    let observed = Arc::new(ObservedStore::new(Arc::clone(&inner), CountMode::Fixed(1)));
    let adapter = Arc::new(FakeAdapter::default());
    let service = limited(
        Arc::clone(&observed) as Arc<dyn SessionStore>,
        Arc::clone(&adapter),
        1,
    );

    let error = service
        .create_session(&user("owner"), spec())
        .await
        .expect_err("the only slot is already occupied");

    assert_eq!(error, CoreError::SessionLimitReached { limit: 1, live: 1 });
    assert!(inner.sessions.lock().await.is_empty());
    assert!(adapter.last_spec.lock().await.is_none());
    assert_eq!(observed.calls(), 1);
}

#[tokio::test]
async fn creation_below_the_limit_succeeds_until_the_last_slot_is_taken() {
    let store = Arc::new(FakeStore::default());
    let service = limited(
        Arc::clone(&store) as Arc<dyn SessionStore>,
        Arc::new(FakeAdapter::default()),
        2,
    );
    let owner = user("owner");

    service
        .create_session(&owner, spec())
        .await
        .expect("the first slot is free");
    service
        .create_session(&owner, spec())
        .await
        .expect("the second slot is free");
    let error = service
        .create_session(&owner, spec())
        .await
        .expect_err("both slots are now occupied");

    assert_eq!(error, CoreError::SessionLimitReached { limit: 2, live: 2 });
    assert_eq!(store.sessions.lock().await.len(), 2);
}

#[tokio::test]
async fn concurrent_creations_share_one_owner_guard_through_the_write() {
    let inner = Arc::new(FakeStore::default());
    let observed = Arc::new(ObservedStore::racing(Arc::clone(&inner)));
    let service = limited(
        Arc::clone(&observed) as Arc<dyn SessionStore>,
        Arc::new(FakeAdapter::default()),
        1,
    );
    let owner = user("owner");

    let (first, second) = tokio::join!(
        service.create_session(&owner, spec()),
        service.create_session(&owner, spec())
    );

    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    assert_eq!(
        usize::from(matches!(
            first,
            Err(CoreError::SessionLimitReached { limit: 1, live: 1 })
        )) + usize::from(matches!(
            second,
            Err(CoreError::SessionLimitReached { limit: 1, live: 1 })
        )),
        1
    );
    assert_eq!(inner.sessions.lock().await.len(), 1);
}

#[tokio::test]
async fn revive_is_refused_when_another_session_occupies_the_slot() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let unlimited = service_with_settings(
        Arc::clone(&store) as Arc<dyn SessionStore>,
        Arc::clone(&adapter),
        settings(None),
    );
    let owner = user("owner");
    let running = unlimited
        .create_session(&owner, spec())
        .await
        .expect("first session creates");
    let stopped = unlimited
        .create_session(&owner, spec())
        .await
        .expect("second session creates before the limit is set");
    await_settled(&unlimited, &owner, &running.id).await;
    await_settled(&unlimited, &owner, &stopped.id).await;
    unlimited
        .stop_session(&owner, &stopped.id)
        .await
        .expect("second session stops");

    let limited = limited(
        Arc::clone(&store) as Arc<dyn SessionStore>,
        Arc::clone(&adapter),
        1,
    );
    let error = limited
        .revive_session(&owner, &stopped.id)
        .await
        .expect_err("the running session occupies the only slot");

    assert_eq!(error, CoreError::SessionLimitReached { limit: 1, live: 1 });
    assert_eq!(
        store
            .get(&stopped.id)
            .await
            .unwrap()
            .unwrap()
            .session
            .status,
        SessionStatus::Idle
    );
}

#[tokio::test]
async fn rereviving_a_live_session_returns_before_the_limit_is_queried() {
    let inner = Arc::new(FakeStore::default());
    inner.disable_coordination();
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("owner");
    let unlimited = service_with_settings(
        Arc::clone(&inner) as Arc<dyn SessionStore>,
        Arc::clone(&adapter),
        settings(None),
    );
    let created = unlimited
        .create_session(&owner, spec())
        .await
        .expect("session creates");
    let ready = await_settled(&unlimited, &owner, &created.id).await;
    let observed = Arc::new(ObservedStore::new(Arc::clone(&inner), CountMode::Fail));
    let limited = limited(Arc::clone(&observed) as Arc<dyn SessionStore>, adapter, 1);

    let returned = limited
        .revive_session(&owner, &created.id)
        .await
        .expect("already live is idempotent");

    assert_eq!(returned.status, ready.status);
    assert_eq!(observed.calls(), 0);
}

#[tokio::test]
async fn the_explicit_exemption_skips_the_count_query() {
    let inner = Arc::new(FakeStore::default());
    let observed = Arc::new(ObservedStore::new(Arc::clone(&inner), CountMode::Fail));
    let service = limited(
        Arc::clone(&observed) as Arc<dyn SessionStore>,
        Arc::new(FakeAdapter::default()),
        1,
    );
    let exempt = Caller::new(
        SubjectId("owner".into()),
        Authorization::from_capabilities([
            Capability::CreateSession,
            Capability::ExceedSessionLimit,
        ]),
    );

    service
        .create_session(&exempt, spec())
        .await
        .expect("the explicit exemption does not count");

    assert_eq!(observed.calls(), 0);
    assert_eq!(inner.sessions.lock().await.len(), 1);
}

#[tokio::test]
async fn a_read_only_operator_grant_is_not_a_limit_exemption() {
    let inner = Arc::new(FakeStore::default());
    let observed = Arc::new(ObservedStore::new(Arc::clone(&inner), CountMode::Fixed(1)));
    let service = limited(
        Arc::clone(&observed) as Arc<dyn SessionStore>,
        Arc::new(FakeAdapter::default()),
        1,
    );
    let reader = Caller::new(
        SubjectId("owner".into()),
        Authorization::from_capabilities([Capability::CreateSession, Capability::ReadActivityLog]),
    );

    assert!(matches!(
        service.create_session(&reader, spec()).await,
        Err(CoreError::SessionLimitReached { limit: 1, live: 1 })
    ));
    assert_eq!(observed.calls(), 1);
}

#[tokio::test]
async fn the_default_store_count_uses_recorded_desired_state_and_owner() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let service = service_with_settings(
        Arc::clone(&store) as Arc<dyn SessionStore>,
        adapter,
        settings(None),
    );
    let owner = user("owner");
    let seed = service
        .create_session(&owner, spec())
        .await
        .expect("seed creates");
    let settled = await_settled(&service, &owner, &seed.id).await;
    let template = store.get(&settled.id).await.unwrap().unwrap();

    let mut records = store.sessions.lock().await;
    records.clear();
    for (index, (status, stop_reason)) in [
        (SessionStatus::Provisioning, None),
        (SessionStatus::Ready, None),
        (SessionStatus::Running, None),
        (SessionStatus::Rotating, None),
        (SessionStatus::Idle, None),
        (
            SessionStatus::Idle,
            Some(crate::session::StopReason::RequiresAction),
        ),
        (
            SessionStatus::Terminated,
            Some(crate::session::StopReason::UserTerminated),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let mut record = template.clone();
        record.session.id = SessionId(format!("sesn_owner_{index}"));
        record.session.status = status;
        record.session.stop_reason = stop_reason;
        records.insert(record.session.id.0.clone(), record);
    }
    let mut other = template;
    other.session.id = SessionId("sesn_other".into());
    other.session.owner = SubjectId("someone-else".into());
    other.session.status = SessionStatus::Ready;
    records.insert(other.session.id.0.clone(), other);
    drop(records);

    assert_eq!(store.count_live_for(&owner.subject).await.unwrap(), 5);
}

#[tokio::test]
async fn reconciliation_restores_every_live_record_without_counting() {
    let inner = Arc::new(FakeStore::default());
    inner.disable_coordination();
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("owner");
    let first = service_with_settings(
        Arc::clone(&inner) as Arc<dyn SessionStore>,
        Arc::clone(&adapter),
        settings(None),
    );
    let mut ids = Vec::new();
    for _ in 0..3 {
        let created = first
            .create_session(&owner, spec())
            .await
            .expect("session creates before the limit is set");
        await_settled(&first, &owner, &created.id).await;
        ids.push(created.id);
    }
    drop(first);

    let observed = Arc::new(ObservedStore::new(Arc::clone(&inner), CountMode::Fail));
    let restarted = limited(Arc::clone(&observed) as Arc<dyn SessionStore>, adapter, 1);
    let reconciled = restarted.reconcile().await;

    assert_eq!(reconciled.readopted_sessions, 3);
    assert_eq!(observed.calls(), 0);
    for id in ids {
        assert!(inner
            .get(&id)
            .await
            .unwrap()
            .unwrap()
            .session
            .status
            .wants_runtime());
    }
}

#[tokio::test]
async fn unreadable_settings_fall_back_to_no_limit() {
    let store = Arc::new(FakeStore::default());
    let service = service_with_settings(
        Arc::clone(&store) as Arc<dyn SessionStore>,
        Arc::new(FakeAdapter::default()),
        broken_settings(),
    );

    service
        .create_session(&user("owner"), spec())
        .await
        .expect("shipped settings carry no limit");

    assert_eq!(store.sessions.lock().await.len(), 1);
}
