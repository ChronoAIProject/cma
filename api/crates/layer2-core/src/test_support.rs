//! Test doubles shared by the service's test modules.
//!
//! The store and the adapter are the two things a session depends on that are
//! not the core itself, so faking exactly those lets the tests exercise real
//! service code. The store is deliberately shareable between service
//! instances: that is what makes "restart" expressible as a test.

use std::collections::HashMap;
use std::sync::Arc;

use layer3_adapter::{
    AdapterError, KernelAdapter, RuntimeHandle, RuntimeSpec, TermSize, TerminalChannel,
};
use tokio::sync::{mpsc, Mutex};

use crate::authz::{Authorization, Caller, Capability};
use crate::event::Event;
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
    pub(crate) events: Mutex<Vec<Event>>,
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
        self.events.lock().await.extend_from_slice(events);
        Ok(())
    }

    async fn events_since(&self, id: &SessionId, after: u64) -> Result<Vec<Event>, StoreError> {
        self.guard().await?;
        let mut out: Vec<Event> = self
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

/// An adapter that provisions nothing and records what it was asked to tear
/// down, so teardown can be asserted without a sandbox.
#[derive(Default)]
pub(crate) struct FakeAdapter {
    pub(crate) terminated: Mutex<Vec<String>>,
    /// When set, provisioning fails. Models a provider that cannot give us a
    /// runtime at all.
    pub(crate) refuse_provision: bool,
    /// How many attempts fail before one succeeds. Models the transient
    /// failure that retrying exists for.
    pub(crate) fail_first: std::sync::atomic::AtomicUsize,
    /// When set, provisioning succeeds and attaching does not. This is the
    /// case that leaks: a runtime exists that nothing has a handle to yet.
    pub(crate) refuse_attach: bool,
    /// Kept alive so the relay's output channel does not close immediately,
    /// which would make a live session look dead.
    pub(crate) senders: Mutex<Vec<mpsc::Sender<Vec<u8>>>>,
    /// What the provider currently has running: handle to the session it was
    /// provisioned for. This is the provider's own view, which is the thing
    /// reconciliation compares the record against.
    pub(crate) running: Mutex<HashMap<String, Option<String>>>,
    /// When set, enumeration fails. Models a provider that cannot be asked,
    /// which must never be read as a provider running nothing.
    pub(crate) refuse_list: bool,
    /// Handles issued so far. Monotonic rather than a count of what is running,
    /// so a replacement sandbox is never named after the one it replaced.
    pub(crate) issued: std::sync::atomic::AtomicUsize,
    /// What each runtime says its agent's conversation is.
    pub(crate) conversations: Mutex<HashMap<String, Vec<u8>>>,
    /// Conversations handed to newly provisioned runtimes, in order.
    pub(crate) handovers: Mutex<Vec<Vec<u8>>>,
    /// When set, the runtime cannot hand its conversation over.
    pub(crate) refuse_snapshot: bool,
}

impl FakeAdapter {
    /// Forget a runtime without being told to, as a provider does when a node
    /// is replaced underneath a sandbox.
    pub(crate) async fn lose(&self, handle: &str) {
        self.running.lock().await.remove(handle);
    }

    pub(crate) async fn running_count(&self) -> usize {
        self.running.lock().await.len()
    }

    /// Give a runtime a conversation to hand over when asked.
    pub(crate) async fn set_conversation(&self, handle: &str, bytes: &[u8]) {
        self.conversations
            .lock()
            .await
            .insert(handle.to_string(), bytes.to_vec());
    }
}

#[async_trait::async_trait]
impl KernelAdapter for FakeAdapter {
    fn kernel_name(&self) -> &'static str {
        "fake"
    }

    async fn provision(&self, spec: &RuntimeSpec) -> Result<RuntimeHandle, AdapterError> {
        if self.refuse_provision {
            return Err(AdapterError::Unavailable("no capacity".into()));
        }
        // Fail the first few attempts, then succeed, as a provider under load
        // does.
        if self
            .fail_first
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |n| if n > 0 { Some(n - 1) } else { None },
            )
            .is_ok()
        {
            return Err(AdapterError::Unavailable("provider busy".into()));
        }
        // Unique per provisioning, not per session: a session given a second
        // runtime must be distinguishable from one still on its first, which is
        // the whole question reconciliation answers.
        let n = self
            .issued
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        if let Some(carried) = spec.restore_from.clone() {
            self.handovers.lock().await.push(carried);
        }
        let mut running = self.running.lock().await;
        let handle = format!("runtime-{n}-for-{}", spec.session_id);
        running.insert(handle.clone(), Some(spec.session_id.clone()));
        Ok(RuntimeHandle(handle))
    }

    async fn attach_terminal(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<TerminalChannel, AdapterError> {
        if self.refuse_attach {
            return Err(AdapterError::Unavailable("terminal not listening".into()));
        }
        // A sandbox that is not running cannot be attached to. Without this the
        // fake would let a session reattach to something that no longer exists,
        // which is the failure these tests are about.
        if !self.running.lock().await.contains_key(&handle.0) {
            return Err(AdapterError::Unavailable("no such runtime".into()));
        }
        let (out_tx, out_rx) = mpsc::channel(8);
        let (in_tx, _in_rx) = mpsc::channel(8);
        self.senders.lock().await.push(out_tx);
        // The input receiver is dropped: nothing in these tests reads it, and
        // the service tolerates a closed input channel by design.
        Ok(TerminalChannel {
            output: out_rx,
            input: in_tx,
        })
    }

    async fn resize(&self, _handle: &RuntimeHandle, _size: TermSize) -> Result<(), AdapterError> {
        Ok(())
    }

    async fn terminate(&self, handle: &RuntimeHandle) -> Result<(), AdapterError> {
        self.terminated.lock().await.push(handle.0.clone());
        self.running.lock().await.remove(&handle.0);
        Ok(())
    }

    async fn snapshot(&self, handle: &RuntimeHandle) -> Result<Option<Vec<u8>>, AdapterError> {
        if self.refuse_snapshot {
            return Err(AdapterError::Unsupported("cannot capture".into()));
        }
        Ok(self.conversations.lock().await.get(&handle.0).cloned())
    }

    async fn list_runtimes(&self) -> Result<Vec<(RuntimeHandle, Option<String>)>, AdapterError> {
        if self.refuse_list {
            return Err(AdapterError::Unsupported("cannot enumerate".into()));
        }
        Ok(self
            .running
            .lock()
            .await
            .iter()
            .map(|(h, s)| (RuntimeHandle(h.clone()), s.clone()))
            .collect())
    }
}

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

/// A screen that keeps everything it is given.
///
/// The null screen is right for tests about lifecycle, where the content is
/// irrelevant. It is exactly wrong for tests about whether content survives,
/// which would pass against a model that never held anything.
#[derive(Default)]
pub(crate) struct RecordingScreen {
    geometry: TerminalGeometry,
    bytes: Vec<u8>,
}

impl ScreenModel for RecordingScreen {
    fn apply(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }
    fn resize(&mut self, geometry: TerminalGeometry) {
        self.geometry = geometry;
    }
    fn snapshot(&self) -> ScreenSnapshot {
        ScreenSnapshot {
            geometry: self.geometry,
            rows: Vec::new(),
        }
    }
    fn repaint_bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }
}

/// A credential holder that always has the same one.
pub(crate) struct FixedCredential(pub(crate) String);

#[async_trait::async_trait]
impl crate::store::CredentialSource for FixedCredential {
    async fn credential_for(&self, _subject: &SubjectId) -> Option<String> {
        Some(self.0.clone())
    }
}

/// A service that stores transcripts, over a screen model that keeps them.
pub(crate) fn service_with_context(
    store: Arc<FakeStore>,
    adapter: Arc<FakeAdapter>,
    context: Arc<dyn crate::store::ContextStore>,
) -> Arc<SessionService> {
    let factory: ScreenFactory = Arc::new(|_| Box::new(RecordingScreen::default()));
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(kernel(), adapter);
    let service = SessionService::new(adapters, factory, store, Some(context), None, None);
    service
        .retry_backoff_ms
        .store(1, std::sync::atomic::Ordering::Relaxed);
    service
}

/// A service that expects a particular runtime version.
///
/// Two of these over one store is an upgrade: the second expects a version the
/// first's sessions are not on.
pub(crate) fn service_on_version(
    store: Arc<FakeStore>,
    adapter: Arc<FakeAdapter>,
    version: &str,
) -> Arc<SessionService> {
    let factory: ScreenFactory = Arc::new(|_| Box::new(RecordingScreen::default()));
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(kernel(), adapter);
    let service = SessionService::new(
        adapters,
        factory,
        store,
        None,
        None,
        Some(version.to_string()),
    );
    service
        .retry_backoff_ms
        .store(1, std::sync::atomic::Ordering::Relaxed);
    service
}

/// A service that can supply an owner's credential without the owner present.
pub(crate) fn service_with_credentials(
    store: Arc<FakeStore>,
    adapter: Arc<FakeAdapter>,
    credential: &str,
) -> Arc<SessionService> {
    let factory: ScreenFactory = Arc::new(|g| Box::new(NullScreen(g)));
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(kernel(), adapter);
    let service = SessionService::new(
        adapters,
        factory,
        store,
        None,
        Some(Arc::new(FixedCredential(credential.to_string()))),
        None,
    );
    service
        .retry_backoff_ms
        .store(1, std::sync::atomic::Ordering::Relaxed);
    service
}

/// Put bytes on a session's screen, as its runtime would.
pub(crate) async fn write_screen(svc: &Arc<SessionService>, id: &SessionId, bytes: &[u8]) {
    let entries = svc.entries.read().await;
    let entry = entries.get(id).expect("the session is live");
    entry.screen.lock().await.apply(bytes);
}

/// The credential a live session is currently holding.
pub(crate) async fn credential_on(svc: &Arc<SessionService>, id: &SessionId) -> Option<String> {
    svc.entries
        .read()
        .await
        .get(id)
        .and_then(|e| e.subject_credential.clone())
}

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
    let service = SessionService::new(adapters, factory, store, None, None, None);
    // Retries are exercised here, not waited out: the production delay exists
    // for a provider under load, which no test is.
    service
        .retry_backoff_ms
        .store(1, std::sync::atomic::Ordering::Relaxed);
    service
}

pub(crate) fn user(subject: &str) -> Caller {
    Caller::new(
        SubjectId(subject.to_string()),
        Authorization::from_capabilities([
            Capability::CreateSession,
            Capability::UseOwnSessions,
            Capability::TerminateOwnSession,
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
        kernel: kernel(),
        geometry: None,
        subject_credential: None,
        options: Default::default(),
    }
}
