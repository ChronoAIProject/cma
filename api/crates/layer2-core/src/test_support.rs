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
use crate::store::{Page, PageCursor, SessionListQuery, SessionRecord, SessionStore, StoreError};
use crate::terminal::{ScreenModel, ScreenSnapshot, TerminalGeometry};
use crate::Reserved;

/// A store held in memory, shared between service instances by `Arc`.
///
/// Stands in for the database precisely where it matters: it is the only thing
/// two service instances have in common, so anything a restarted service can
/// see must have gone through it.
#[derive(Default)]
pub(crate) struct FakeStore {
    /// Makes multi-collection operations atomic to every fake-store reader.
    transaction: Mutex<()>,
    pub(crate) sessions: Mutex<HashMap<String, SessionRecord>>,
    pub(crate) events: Mutex<Vec<StoredEvent>>,
    /// Durable receipts for whole record/event transactions, including empty batches.
    write_receipts: Mutex<HashMap<String, (SessionId, u64)>>,
    /// Successful record writes, so idempotent migrations can be asserted.
    pub(crate) upserts: std::sync::atomic::AtomicUsize,
    /// When set, every operation fails, so callers can be checked against a
    /// store that is down rather than merely empty.
    pub(crate) broken: Mutex<bool>,
    /// When set, durable history alone is unavailable.
    ///
    /// Reconciliation can still read the desired session record and the
    /// provider can still report its runtime alive. This isolates the case
    /// where readoption must defer rather than mistake an incomplete rebuild
    /// for a lost sandbox.
    pub(crate) broken_event_reads: Mutex<bool>,
}

impl FakeStore {
    /// Install an exact record for a fixture, outside the production store port.
    pub(crate) async fn seed(&self, record: &SessionRecord) -> Result<(), StoreError> {
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        self.upserts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.sessions
            .lock()
            .await
            .insert(record.session.id.0.clone(), record.clone());
        Ok(())
    }

    pub(crate) async fn fail_everything(&self) {
        *self.broken.lock().await = true;
    }

    async fn guard(&self) -> Result<(), StoreError> {
        if *self.broken.lock().await {
            return Err(StoreError::Unavailable("test store is down".into()));
        }
        Ok(())
    }

    fn event_tail(held: &[StoredEvent], id: &SessionId) -> u64 {
        held.iter()
            .filter(|event| &event.session_id == id)
            .map(|event| event.seq)
            .max()
            .unwrap_or(0)
    }

    fn validate_append(
        held: &[StoredEvent],
        events: &[Event],
        recorded_tail: u64,
    ) -> Result<(), StoreError> {
        let Some(first) = events.first() else {
            return Ok(());
        };
        let id = &first.session_id;
        let mut expected = Self::event_tail(held, id)
            .max(recorded_tail)
            .checked_add(1)
            .ok_or_else(|| StoreError::Malformed(format!("event sequence overflow for {id}")))?;

        for event in events {
            if event.session_id != *id {
                return Err(StoreError::Malformed(
                    "one event append batch named more than one session".into(),
                ));
            }
            if event.seq < expected {
                return Err(StoreError::SequenceTaken {
                    expected,
                    got: event.seq,
                });
            }
            if event.seq > expected {
                return Err(StoreError::NonContiguousAppend {
                    expected,
                    got: event.seq,
                });
            }
            expected = expected.checked_add(1).ok_or_else(|| {
                StoreError::Malformed(format!("event sequence overflow for {id}"))
            })?;
        }
        Ok(())
    }

    async fn commit_idempotent(
        &self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
        create: bool,
    ) -> Result<u64, StoreError> {
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        let mut receipts = self.write_receipts.lock().await;
        if let Some((held_by, revision)) = receipts.get(write_id) {
            if held_by != &record.session.id {
                return Err(StoreError::Malformed(format!(
                    "session write ID {write_id} belongs to another session"
                )));
            }
            return Ok(*revision);
        }

        let mut sessions = self.sessions.lock().await;
        let existing = sessions.get(&record.session.id.0);
        let actual = existing.map_or(0, |stored| stored.revision);
        let recorded_tail = existing.map_or(0, |stored| stored.session.last_seq);
        if create {
            if record.revision != 0 || existing.is_some() {
                return Err(StoreError::RevisionConflict {
                    expected: 0,
                    actual,
                });
            }
        } else {
            if existing.is_none() {
                return Err(StoreError::SessionDeleted {
                    id: record.session.id.clone(),
                });
            }
            if actual != record.revision {
                return Err(StoreError::RevisionConflict {
                    expected: record.revision,
                    actual,
                });
            }
            if existing.is_some_and(|stored| {
                stored.session.status.is_terminal() && !record.session.status.is_terminal()
            }) {
                return Err(StoreError::SessionTerminated {
                    id: record.session.id.clone(),
                });
            }
        }

        let mut held = self.events.lock().await;
        let sequence_floor = Self::event_tail(&held, &record.session.id).max(recorded_tail);
        Self::validate_append(&held, events, recorded_tail)?;
        let requested_tail = record.session.last_seq;
        let durable_tail = events.last().map_or(sequence_floor, |event| event.seq);
        let tail_matches = if events.is_empty() {
            requested_tail <= sequence_floor
        } else {
            requested_tail == durable_tail
        };
        if !tail_matches {
            return Err(StoreError::Malformed(format!(
                "session {} record tail {} does not match durable tail {}",
                record.session.id, requested_tail, durable_tail
            )));
        }
        let revision = actual
            .checked_add(1)
            .ok_or_else(|| StoreError::Malformed("session revision overflow".into()))?;
        let mut durable = record.clone();
        durable.revision = revision;
        durable.session.last_seq = durable_tail;
        held.extend(events.iter().cloned().map(StoredEvent::from));
        sessions.insert(record.session.id.0.clone(), durable);
        receipts.insert(write_id.to_string(), (record.session.id.clone(), revision));
        self.upserts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(revision)
    }
}

#[async_trait::async_trait]
impl SessionStore for FakeStore {
    async fn ping(&self) -> Result<(), StoreError> {
        self.guard().await
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        Ok(self.sessions.lock().await.get(&id.0).cloned())
    }

    async fn list_for(
        &self,
        owner: &SubjectId,
        query: &SessionListQuery,
    ) -> Result<Page<SessionRecord>, StoreError> {
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        Ok(page_sessions(
            self.sessions
                .lock()
                .await
                .values()
                .filter(|r| &r.session.owner == owner)
                .cloned()
                .collect(),
            query,
        ))
    }

    async fn list_all(&self, query: &SessionListQuery) -> Result<Page<SessionRecord>, StoreError> {
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        Ok(page_sessions(
            self.sessions.lock().await.values().cloned().collect(),
            query,
        ))
    }

    async fn reserved_capacity(&self) -> Result<Reserved, StoreError> {
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        let mut reserved = Reserved::default();
        for record in self.sessions.lock().await.values() {
            if record.session.wants_runtime() {
                reserved.include(record.session.options.size);
            }
        }
        Ok(reserved)
    }

    async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        self.sessions.lock().await.remove(&id.0);
        self.events.lock().await.retain(|e| &e.session_id != id);
        self.write_receipts
            .lock()
            .await
            .retain(|_, (session_id, _)| session_id != id);
        Ok(())
    }

    /// Append atomically, refusing a collision or a forward gap.
    ///
    /// The refusal is the point. This double accepted duplicates, so a race
    /// that the database rejects outright looked perfectly healthy here — the
    /// suite could not see the defect that dropped a turn's events in
    /// production, because the only thing that would have caught it was the
    /// unique index this stands in for. A double that is more permissive than
    /// the thing it doubles is a test that passes for the wrong reason.
    ///
    async fn append_events(&self, events: &[Event]) -> Result<(), StoreError> {
        self.guard().await?;
        let _transaction = self.transaction.lock().await;
        let recorded_tail = match events.first() {
            Some(first) => self
                .sessions
                .lock()
                .await
                .get(&first.session_id.0)
                .map_or(0, |record| record.session.last_seq),
            None => 0,
        };
        let mut held = self.events.lock().await;
        Self::validate_append(&held, events, recorded_tail)?;
        held.extend(events.iter().cloned().map(StoredEvent::from));
        Ok(())
    }

    async fn commit_session_idempotent(
        &self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError> {
        self.commit_idempotent(write_id, record, events, false)
            .await
    }

    async fn create_session_idempotent(
        &self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError> {
        self.commit_idempotent(write_id, record, events, true).await
    }

    async fn event_tail(&self, id: &SessionId) -> Result<u64, StoreError> {
        self.guard().await?;
        Ok(self
            .events
            .lock()
            .await
            .iter()
            .filter(|event| &event.session_id == id)
            .map(|event| event.seq)
            .max()
            .unwrap_or(0))
    }

    async fn events_since(
        &self,
        id: &SessionId,
        after: u64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        self.guard().await?;
        if *self.broken_event_reads.lock().await {
            return Err(StoreError::Unavailable("test event history is down".into()));
        }
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

fn page_sessions(mut records: Vec<SessionRecord>, query: &SessionListQuery) -> Page<SessionRecord> {
    records.retain(|record| {
        query
            .status
            .is_none_or(|status| record.session.status == status)
            && query.page.cursor().is_none_or(|cursor| {
                record.session.created_at < cursor.created_at
                    || (record.session.created_at == cursor.created_at
                        && record.session.id.0 < cursor.id)
            })
    });
    records.sort_by(|a, b| {
        b.session
            .created_at
            .cmp(&a.session.created_at)
            .then_with(|| b.session.id.0.cmp(&a.session.id.0))
    });
    let has_more = records.len() > query.page.limit() as usize;
    records.truncate(query.page.limit() as usize);
    let next = has_more.then(|| {
        let last = records
            .last()
            .expect("a non-empty bounded page has a last row");
        PageCursor::new(&last.session.created_at, &last.session.id.0)
    });
    Page {
        items: records,
        next,
    }
}

pub(crate) use crate::adapter_support::FakeAdapter;
pub(crate) use crate::environment_support::{
    baseline, catalog_service, environments, fake_agent, profile_spec, unconfigured_settings,
    FakeProfileStore, FakeSkillCatalog, ReplacementInterference,
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
    credential_on, owned, service_as, service_on_version, service_on_version_with_settings,
    service_on_version_with_skills, service_over_with_context, service_with_context,
    service_with_credentials, service_with_environments_and_settings,
    service_with_environments_and_settings_service, service_with_environments_on_version,
    write_screen,
};

pub(crate) fn kernel() -> KernelId {
    KernelId::new("fake")
}

/// A service instance over the given store and adapter.
///
/// Calling this twice with the same store is the restart: the second instance
/// shares the durable record and nothing else.
pub(crate) fn service(store: Arc<FakeStore>, adapter: Arc<FakeAdapter>) -> Arc<SessionService> {
    service_over(store, adapter)
}

/// The same, over any store at all.
///
/// Taken as the trait rather than the double so a test can stand a badly
/// behaved implementation in front of the service — a store is a port, and how
/// the core survives one that misbehaves is worth being able to say.
pub(crate) fn service_over(
    store: Arc<dyn SessionStore>,
    adapter: Arc<FakeAdapter>,
) -> Arc<SessionService> {
    let factory: ScreenFactory = Arc::new(|g| Box::new(NullScreen(g)));
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(kernel(), adapter);
    let service = SessionService::new(
        adapters,
        factory,
        store,
        crate::service::SessionPorts {
            context: None,
            credentials: None,
            skills: None,
            workspaces: None,
            settings: None,
        },
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
    service_over_with_environments(store, adapter, profiles)
}

/// The environment-aware fixture over an adversarial session store.
pub(crate) fn service_over_with_environments(
    store: Arc<dyn SessionStore>,
    adapter: Arc<FakeAdapter>,
    profiles: Arc<EnvironmentProfileService>,
) -> Arc<SessionService> {
    let factory: ScreenFactory = Arc::new(|g| Box::new(NullScreen(g)));
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(kernel(), adapter);
    SessionService::new(
        adapters,
        factory,
        store,
        crate::service::SessionPorts {
            context: None,
            credentials: None,
            skills: None,
            workspaces: None,
            settings: None,
        },
        profiles,
        owned(),
    )
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
        unconfigured_settings(),
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
        unconfigured_settings(),
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
/// asserts on a running runtime has to wait for one. Polls the store and the
/// in-process bring-up claim rather than sleeping a fixed time, so it is
/// neither flaky nor slower than the thing it waits for. A successful start
/// records `Ready` before publishing its live entry, and the claim spans both
/// boundaries.
pub(crate) async fn await_settled(
    svc: &Arc<SessionService>,
    caller: &Caller,
    id: &crate::ids::SessionId,
) -> crate::session::Session {
    for _ in 0..400 {
        if let Ok(s) = svc.get_session(caller, id).await {
            if s.status != crate::session::SessionStatus::Provisioning
                && !svc.starting.read().await.contains(id)
            {
                return s;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("session never finished starting");
}

pub(crate) fn spec() -> SessionSpec {
    SessionSpec {
        geometry: None,
        environment_profile: None,
        caller_credential: None,
        runtime_credential: None,
        requested_mode: None,
        quoted_rate_card: None,
        options: Default::default(),
    }
}
