//! Test doubles shared by the service's test modules.
//!
//! The store and the adapter are the two things a session depends on that are
//! not the core itself, so faking exactly those lets the tests exercise real
//! service code. The store is deliberately shareable between service
//! instances: that is what makes "restart" expressible as a test.

use std::collections::HashMap;
use std::sync::Arc;

use layer3_adapter::KernelAdapter;
use tokio::sync::Mutex;

use crate::authz::{Authorization, Caller, Capability};
use crate::environment_service::EnvironmentProfileService;
use crate::event::{Event, StoredEvent};
use crate::ids::{KernelId, SessionId, SubjectId};
use crate::service::{ScreenFactory, SessionService};
use crate::session::SessionSpec;
use crate::store::{SessionRecord, SessionStore, StoreError};
use crate::terminal::{ScreenModel, ScreenSnapshot, TerminalGeometry};

/// A store held in memory, shared between service instances by `Arc`.
///
/// Stands in for the database precisely where it matters: it is the only thing
/// two service instances have in common, so anything a restarted service can
/// see must have gone through it.
#[derive(Default)]
pub(crate) struct FakeStore {
    pub(crate) sessions: Mutex<HashMap<String, SessionRecord>>,
    pub(crate) events: Mutex<Vec<StoredEvent>>,
    /// When set, every operation fails, so callers can be checked against a
    /// store that is down rather than merely empty.
    pub(crate) broken: Mutex<bool>,
}

impl FakeStore {
    pub(crate) async fn fail_everything(&self) {
        *self.broken.lock().await = true;
    }

    async fn guard(&self) -> Result<(), StoreError> {
        if *self.broken.lock().await {
            return Err(StoreError::Unavailable("test store is down".into()));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl SessionStore for FakeStore {
    async fn upsert(&self, record: &SessionRecord) -> Result<(), StoreError> {
        self.guard().await?;
        self.sessions
            .lock()
            .await
            .insert(record.session.id.0.clone(), record.clone());
        Ok(())
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
        self.guard().await?;
        Ok(self.sessions.lock().await.get(&id.0).cloned())
    }

    async fn list_for(&self, owner: &SubjectId) -> Result<Vec<SessionRecord>, StoreError> {
        self.guard().await?;
        Ok(self
            .sessions
            .lock()
            .await
            .values()
            .filter(|r| &r.session.owner == owner)
            .cloned()
            .collect())
    }

    async fn list_all(&self) -> Result<Vec<SessionRecord>, StoreError> {
        self.guard().await?;
        Ok(self.sessions.lock().await.values().cloned().collect())
    }

    async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
        self.guard().await?;
        self.sessions.lock().await.remove(&id.0);
        self.events.lock().await.retain(|e| &e.session_id != id);
        Ok(())
    }

    async fn append_events(&self, events: &[Event]) -> Result<(), StoreError> {
        self.guard().await?;
        self.events
            .lock()
            .await
            .extend(events.iter().cloned().map(StoredEvent::from));
        Ok(())
    }

    async fn events_since(
        &self,
        id: &SessionId,
        after: u64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        self.guard().await?;
        let mut out: Vec<StoredEvent> = self
            .events
            .lock()
            .await
            .iter()
            .filter(|e| &e.session_id == id && e.seq > after)
            .cloned()
            .collect();
        out.sort_by_key(|e| e.seq);
        Ok(out)
    }
}

pub(crate) use crate::adapter_support::FakeAdapter;
pub(crate) use crate::environment_support::{
    baseline, catalog_service, environments, profile_spec, FakeProfileStore, FakeSkillCatalog,
};
pub(crate) use crate::screen_support::{FixedCredential, RecordingScreen};

pub(crate) struct NullScreen(pub(crate) TerminalGeometry);

impl ScreenModel for NullScreen {
    fn apply(&mut self, _bytes: &[u8]) {}
    fn resize(&mut self, geometry: TerminalGeometry) {
        self.0 = geometry;
    }
    fn snapshot(&self) -> ScreenSnapshot {
        ScreenSnapshot {
            geometry: self.0,
            rows: Vec::new(),
        }
    }
    fn repaint_bytes(&self) -> Vec<u8> {
        Vec::new()
    }
}

pub(crate) use crate::service_support::{
    credential_on, owned, service_as, service_on_version, service_with_context,
    service_with_credentials, write_screen,
};

pub(crate) fn kernel() -> KernelId {
    KernelId::new("fake")
}

/// A service instance over the given store and adapter.
///
/// Calling this twice with the same store is the restart: the second instance
/// shares the durable record and nothing else.
pub(crate) fn service(store: Arc<FakeStore>, adapter: Arc<FakeAdapter>) -> Arc<SessionService> {
    let factory: ScreenFactory = Arc::new(|g| Box::new(NullScreen(g)));
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(kernel(), adapter);
    let service = SessionService::new(
        adapters,
        factory,
        store,
        None,
        None,
        no_environments(),
        owned(),
    );
    // Retries are exercised here, not waited out: the production delay exists
    // for a provider under load, which no test is.
    service
        .retry_backoff_ms
        .store(1, std::sync::atomic::Ordering::Relaxed);
    service
}

/// A service instance over a particular set of environment profiles.
pub(crate) fn service_with_environments(
    store: Arc<FakeStore>,
    adapter: Arc<FakeAdapter>,
    profiles: Arc<EnvironmentProfileService>,
) -> Arc<SessionService> {
    let factory: ScreenFactory = Arc::new(|g| Box::new(NullScreen(g)));
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(kernel(), adapter);
    SessionService::new(adapters, factory, store, None, None, profiles, owned())
}

/// A profile service with nothing in it, for tests that do not care.
///
/// Every session resolves an environment, so one always has to be present;
/// this is the one that adds nothing to what the deployment mandates.
pub(crate) fn no_environments() -> Arc<EnvironmentProfileService> {
    EnvironmentProfileService::new(
        Arc::new(FakeProfileStore::default()),
        baseline(),
        None,
        None,
        crate::environment::AgentConfiguration {
            // The kernel the fake adapter is registered under. The default is
            // the one this deployment ships, and a fixture that left it there
            // would be asking for a runtime the test never registered.
            kernel: KernelId::new("fake"),
            ..Default::default()
        },
    )
}

/// A profile service whose deployment default pins these agent settings.
///
/// For tests about what a session is started with, now that the answer comes
/// from the deployment or a profile rather than from the request.
pub(crate) fn environments_defaulting_to(
    agent: crate::environment::AgentConfiguration,
) -> Arc<EnvironmentProfileService> {
    EnvironmentProfileService::new(
        Arc::new(FakeProfileStore::default()),
        baseline(),
        None,
        None,
        agent,
    )
}

pub(crate) fn user(subject: &str) -> Caller {
    Caller::new(
        SubjectId(subject.to_string()),
        Authorization::from_capabilities([
            Capability::CreateSession,
            Capability::UseOwnSessions,
            Capability::TerminateOwnSession,
            Capability::ManageOwnEnvironmentProfiles,
        ]),
    )
}

pub(crate) fn admin() -> Caller {
    Caller::new(
        SubjectId("admin".into()),
        Authorization::from_capabilities([Capability::ReadAllSessions]),
    )
}

/// Wait until a session has finished starting, whichever way it went.
///
/// Creating a session now returns before its runtime exists, so a test that
/// asserts on a running runtime has to wait for one. Polls the store rather
/// than sleeping a fixed time, so it is neither flaky nor slower than the
/// thing it waits for.
pub(crate) async fn await_settled(
    svc: &Arc<SessionService>,
    caller: &Caller,
    id: &crate::ids::SessionId,
) -> crate::session::Session {
    for _ in 0..400 {
        if let Ok(s) = svc.get_session(caller, id).await {
            if s.status != crate::session::SessionStatus::Provisioning {
                return s;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("session never left provisioning");
}

pub(crate) fn spec() -> SessionSpec {
    SessionSpec {
        geometry: None,
        environment_profile: None,
        subject_credential: None,
        options: Default::default(),
    }
}
