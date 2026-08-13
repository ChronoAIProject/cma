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
    /// When set, provisioning succeeds and attaching does not. This is the
    /// case that leaks: a runtime exists that nothing has a handle to yet.
    pub(crate) refuse_attach: bool,
    /// Kept alive so the relay's output channel does not close immediately,
    /// which would make a live session look dead.
    pub(crate) senders: Mutex<Vec<mpsc::Sender<Vec<u8>>>>,
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
        Ok(RuntimeHandle(format!("runtime-for-{}", spec.session_id)))
    }

    async fn attach_terminal(
        &self,
        _handle: &RuntimeHandle,
    ) -> Result<TerminalChannel, AdapterError> {
        if self.refuse_attach {
            return Err(AdapterError::Unavailable("terminal not listening".into()));
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
        Ok(())
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
    SessionService::new(adapters, factory, store, None)
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
    }
}
