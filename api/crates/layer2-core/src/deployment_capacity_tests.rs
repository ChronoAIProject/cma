//! Deployment-wide reservation behavior at creation and across recovery paths.

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use layer3_adapter::KernelAdapter;

use crate::environment::AgentConfiguration;
use crate::event::{Event, StoredEvent};
use crate::ids::{KernelId, SessionId, SubjectId};
use crate::service::{ScreenFactory, SessionPorts, SessionService};
use crate::session::SessionStatus;
use crate::store::{
    PlatformSettingsStore, SessionRecord, SessionStore, StoreError, StoredSettings,
};
use crate::test_support::{
    await_settled, environments_defaulting_to, no_environments, owned, service, service_on_version,
    service_on_version_with_settings, service_with_environments_and_settings, spec, user,
    FakeAdapter, FakeStore, NullScreen,
};
use crate::{
    CoreError, DeploymentCapacity, PlatformSettings, PlatformSettingsService, Reserved, SessionSize,
};

fn size(cpu_millis: u32, memory_mib: u32) -> SessionSize {
    SessionSize::new(cpu_millis, memory_mib).expect("test size is non-zero")
}

fn settings(
    cpu_millis: Option<u64>,
    memory_mib: Option<u64>,
    assumed_session_size: Option<SessionSize>,
) -> PlatformSettings {
    PlatformSettings {
        deployment_capacity: DeploymentCapacity {
            cpu_millis,
            memory_mib,
            assumed_session_size,
        },
        ..PlatformSettings::default()
    }
}

fn sized_environments(session_size: SessionSize) -> Arc<crate::EnvironmentProfileService> {
    environments_defaulting_to(AgentConfiguration {
        kernel: crate::test_support::kernel(),
        size: Some(session_size),
        ..AgentConfiguration::default()
    })
}

struct FixedSettings(PlatformSettings);

#[async_trait::async_trait]
impl PlatformSettingsStore for FixedSettings {
    async fn load(&self) -> Result<Option<StoredSettings>, StoreError> {
        Ok(Some(StoredSettings {
            settings: self.0.clone(),
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

fn service_over(
    store: Arc<dyn SessionStore>,
    adapter: Arc<FakeAdapter>,
    environments: Arc<crate::EnvironmentProfileService>,
    settings: PlatformSettings,
) -> Arc<SessionService> {
    let factory: ScreenFactory = Arc::new(|geometry| Box::new(NullScreen(geometry)));
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(crate::test_support::kernel(), adapter);
    let service = SessionService::new(
        adapters,
        factory,
        store,
        SessionPorts {
            settings: Some(Arc::new(PlatformSettingsService::new(Arc::new(
                FixedSettings(settings),
            )))),
            ..SessionPorts::default()
        },
        environments,
        owned(),
    );
    service.retry_backoff_ms.store(1, Ordering::Relaxed);
    service
}

/// A store that exposes whether the reservation query was reached and widens a
/// write race enough for two concurrent creation requests to meet it.
struct ObservedStore {
    inner: Arc<FakeStore>,
    reserved_calls: AtomicUsize,
    fail_reserved: bool,
    slow_writes: bool,
}

impl ObservedStore {
    fn new(inner: Arc<FakeStore>) -> Self {
        Self {
            inner,
            reserved_calls: AtomicUsize::new(0),
            fail_reserved: false,
            slow_writes: false,
        }
    }

    fn refusing_reservations(inner: Arc<FakeStore>) -> Self {
        Self {
            fail_reserved: true,
            ..Self::new(inner)
        }
    }

    fn racing(inner: Arc<FakeStore>) -> Self {
        Self {
            slow_writes: true,
            ..Self::new(inner)
        }
    }
}

#[async_trait::async_trait]
impl SessionStore for ObservedStore {
    async fn ping(&self) -> Result<(), StoreError> {
        self.inner.ping().await
    }

    async fn upsert(&self, record: &SessionRecord) -> Result<(), StoreError> {
        if self.slow_writes {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
        self.inner.upsert(record).await
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
        self.inner.get(id).await
    }

    async fn list_for(&self, owner: &SubjectId) -> Result<Vec<SessionRecord>, StoreError> {
        self.inner.list_for(owner).await
    }

    async fn count_live_for(&self, owner: &SubjectId) -> Result<u32, StoreError> {
        self.inner.count_live_for(owner).await
    }

    async fn reserved_capacity(&self) -> Result<Reserved, StoreError> {
        self.reserved_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_reserved {
            return Err(StoreError::Unavailable(
                "the reservation query must not be reached".into(),
            ));
        }
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

    async fn events_since(
        &self,
        id: &SessionId,
        after: u64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        self.inner.events_since(id, after).await
    }
}

#[tokio::test]
async fn an_over_capacity_creation_is_refused_before_any_new_record_or_runtime() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let one = size(1_000, 1_024);
    let limited = service_with_environments_and_settings(
        Arc::clone(&store),
        Arc::clone(&adapter),
        sized_environments(one),
        settings(Some(500), Some(512), None),
    );
    let error = limited
        .create_session(&user("owner"), spec())
        .await
        .expect_err("the request itself is larger than all declared room");

    assert_eq!(
        error,
        CoreError::DeploymentAtCapacity {
            requested_cpu_millis: 1_000,
            requested_memory_mib: 1_024,
            free_cpu_millis: 500,
            free_memory_mib: 512,
        }
    );
    assert!(store.sessions.lock().await.is_empty());
    assert_eq!(adapter.provision_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_declared_assumption_prices_an_existing_unsized_session_instead_of_zero() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("owner");
    let existing = service(Arc::clone(&store), Arc::clone(&adapter));
    let seed = existing
        .create_session(&owner, spec())
        .await
        .expect("created");
    await_settled(&existing, &owner, &seed.id).await;

    let one = size(1_000, 1_024);
    let limited = service_with_environments_and_settings(
        Arc::clone(&store),
        Arc::clone(&adapter),
        no_environments(),
        settings(Some(1_000), Some(1_024), Some(one)),
    );
    let before_calls = adapter.provision_calls.load(Ordering::SeqCst);
    assert!(matches!(
        limited.create_session(&user("another-owner"), spec()).await,
        Err(CoreError::DeploymentAtCapacity {
            free_cpu_millis: 0,
            free_memory_mib: 0,
            ..
        })
    ));

    assert_eq!(store.sessions.lock().await.len(), 1);
    assert_eq!(adapter.provision_calls.load(Ordering::SeqCst), before_calls);
}

#[tokio::test]
async fn a_creation_with_room_succeeds() {
    let store = Arc::new(FakeStore::default());
    let one = size(1_000, 1_024);
    let service = service_with_environments_and_settings(
        Arc::clone(&store),
        Arc::new(FakeAdapter::default()),
        no_environments(),
        settings(Some(1_000), Some(1_024), Some(one)),
    );

    service
        .create_session(&user("owner"), spec())
        .await
        .expect("the complete declared room is available");
    assert_eq!(store.sessions.lock().await.len(), 1);
}

#[tokio::test]
async fn an_idle_session_consumes_no_declared_room() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("owner");
    let before = service(Arc::clone(&store), Arc::clone(&adapter));
    let stopped = before
        .create_session(&owner, spec())
        .await
        .expect("created");
    await_settled(&before, &owner, &stopped.id).await;
    before
        .stop_session(&owner, &stopped.id)
        .await
        .expect("session pauses");

    let one = size(1_000, 1_024);
    let limited = service_with_environments_and_settings(
        Arc::clone(&store),
        adapter,
        no_environments(),
        settings(Some(1_000), Some(1_024), Some(one)),
    );
    limited
        .create_session(&user("another-owner"), spec())
        .await
        .expect("a paused session releases its reservation");
}

#[tokio::test]
async fn a_deployment_that_declares_nothing_never_queries_or_refuses_capacity() {
    let inner = Arc::new(FakeStore::default());
    let observed = Arc::new(ObservedStore::refusing_reservations(Arc::clone(&inner)));
    let service = service_over(
        Arc::clone(&observed) as Arc<dyn SessionStore>,
        Arc::new(FakeAdapter::default()),
        no_environments(),
        PlatformSettings::default(),
    );

    service
        .create_session(&user("owner"), spec())
        .await
        .expect("absence of a declaration preserves existing behavior");
    assert_eq!(observed.reserved_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn concurrent_creations_cannot_spend_the_same_local_headroom() {
    let inner = Arc::new(FakeStore::default());
    let observed = Arc::new(ObservedStore::racing(Arc::clone(&inner)));
    let one = size(1_000, 1_024);
    let service = service_over(
        Arc::clone(&observed) as Arc<dyn SessionStore>,
        Arc::new(FakeAdapter::default()),
        no_environments(),
        settings(Some(1_000), Some(1_024), Some(one)),
    );
    let owner_a = user("owner-a");
    let owner_b = user("owner-b");
    let (first, second) = tokio::join!(
        service.create_session(&owner_a, spec()),
        service.create_session(&owner_b, spec())
    );

    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    assert_eq!(
        usize::from(matches!(first, Err(CoreError::DeploymentAtCapacity { .. })))
            + usize::from(matches!(
                second,
                Err(CoreError::DeploymentAtCapacity { .. })
            )),
        1
    );
    assert_eq!(inner.sessions.lock().await.len(), 1);
}

#[derive(Clone, Default)]
struct LogBuffer(Arc<StdMutex<Vec<u8>>>);

struct LogWriter(Arc<StdMutex<Vec<u8>>>);

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("log buffer is not poisoned")
            .extend(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(Arc::clone(&self.0))
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_unsized_record_without_an_assumption_leaves_the_check_inert_and_warns() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let existing = service(Arc::clone(&store), Arc::clone(&adapter));
    let owner = user("owner");
    let seed = existing
        .create_session(&owner, spec())
        .await
        .expect("created");
    await_settled(&existing, &owner, &seed.id).await;

    let logs = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(logs.clone())
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let requested = size(500, 512);
    let limited = service_with_environments_and_settings(
        Arc::clone(&store),
        adapter,
        sized_environments(requested),
        settings(Some(500), Some(512), None),
    );

    limited
        .create_session(&user("another-owner"), spec())
        .await
        .expect("an incomplete sum cannot justify a refusal");

    let bytes = logs.0.lock().expect("log buffer").clone();
    let output = String::from_utf8(bytes).expect("utf-8 logs");
    assert!(output.contains("unsized_sessions=1"), "got {output}");
    assert!(
        output.contains("skipping the capacity check"),
        "got {output}"
    );
    assert_eq!(store.sessions.lock().await.len(), 2);
}

#[tokio::test]
async fn sized_records_are_checked_even_when_no_unsized_assumption_exists() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let requested = size(500, 512);
    let environments = sized_environments(requested);
    let before = service_with_environments_and_settings(
        Arc::clone(&store),
        Arc::clone(&adapter),
        Arc::clone(&environments),
        PlatformSettings::default(),
    );
    let owner = user("owner");
    let seed = before
        .create_session(&owner, spec())
        .await
        .expect("created");
    await_settled(&before, &owner, &seed.id).await;

    let limited = service_with_environments_and_settings(
        Arc::clone(&store),
        adapter,
        environments,
        settings(Some(500), Some(512), None),
    );
    assert!(matches!(
        limited.create_session(&user("another"), spec()).await,
        Err(CoreError::DeploymentAtCapacity {
            free_cpu_millis: 0,
            free_memory_mib: 0,
            ..
        })
    ));
}

#[tokio::test]
async fn reserved_capacity_counts_only_the_four_runtime_wanting_states() {
    let store = Arc::new(FakeStore::default());
    assert_eq!(
        store.reserved_capacity().await.unwrap(),
        Reserved::default()
    );

    let service = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let owner = user("owner");
    let seed = service
        .create_session(&owner, spec())
        .await
        .expect("created");
    let settled = await_settled(&service, &owner, &seed.id).await;
    let template = store.get(&settled.id).await.unwrap().unwrap();

    let mut records = store.sessions.lock().await;
    records.clear();
    for (index, (status, session_size)) in [
        (SessionStatus::Provisioning, Some(size(100, 200))),
        (SessionStatus::Ready, None),
        (SessionStatus::Running, Some(size(300, 400))),
        (SessionStatus::Rotating, None),
        (SessionStatus::Idle, Some(size(500, 600))),
        (SessionStatus::Terminated, Some(size(700, 800))),
    ]
    .into_iter()
    .enumerate()
    {
        let mut record = template.clone();
        record.session.id = SessionId(format!("sesn_capacity_{index}"));
        record.session.status = status;
        record.session.options.size = session_size;
        records.insert(record.session.id.0.clone(), record);
    }
    drop(records);

    assert_eq!(
        store.reserved_capacity().await.unwrap(),
        Reserved {
            sessions: 4,
            unsized_sessions: 2,
            cpu_millis: 400,
            memory_mib: 600,
        }
    );
}

#[tokio::test]
async fn reconciliation_restores_every_record_even_above_declared_capacity() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("owner");
    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let mut ids = Vec::new();
    for _ in 0..3 {
        let created = first.create_session(&owner, spec()).await.expect("created");
        await_settled(&first, &owner, &created.id).await;
        ids.push(created.id);
    }
    drop(first);

    let one = size(1_000, 1_024);
    let restarted = service_with_environments_and_settings(
        Arc::clone(&store),
        adapter,
        no_environments(),
        settings(Some(1_000), Some(1_024), Some(one)),
    );
    let reconciled = restarted.reconcile().await;

    assert_eq!(reconciled.readopted_sessions, 3);
    for id in ids {
        assert!(store
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
async fn rotation_completes_while_the_deployment_is_at_capacity() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("owner");
    let old = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = old.create_session(&owner, spec()).await.expect("created");
    await_settled(&old, &owner, &created.id).await;
    drop(old);

    let one = size(1_000, 1_024);
    let replacement = service_on_version_with_settings(
        Arc::clone(&store),
        Arc::clone(&adapter),
        "runtime:2",
        settings(Some(1_000), Some(1_024), Some(one)),
    );
    assert_eq!(replacement.reconcile().await.rotated_sessions, 1);
    for _ in 0..400 {
        if store
            .get(&created.id)
            .await
            .unwrap()
            .unwrap()
            .session
            .status
            == SessionStatus::Ready
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    let rotated = store.get(&created.id).await.unwrap().unwrap();
    assert_eq!(rotated.session.status, SessionStatus::Ready);
    assert_eq!(rotated.session.rotations, 1);
    assert_eq!(adapter.running_count().await, 1);
}

#[test]
fn the_policy_module_contains_only_core_vocabulary() {
    let source = include_str!("deployment_capacity.rs");
    for forbidden in [
        "quota",
        "namespace",
        "kubernetes",
        "Gi",
        "cpu.max",
        "resourceLimits",
    ] {
        assert!(!source.contains(forbidden), "found {forbidden:?}");
    }
}
