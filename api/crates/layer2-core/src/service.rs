//! The single tunnel: session lifecycle, the event log, and terminal relay.
//!
//! Every interface calls this and only this. The service owns one live screen
//! model per session and keeps it fed for as long as the runtime produces
//! output — whether or not a client is attached. A detached session that keeps
//! working is the normal case, and a client that reattaches must find the
//! screen already correct rather than replaying history.

use std::collections::HashMap;
use std::sync::Arc;

use layer3_adapter::{KernelAdapter, RuntimeHandle, TermSize, TerminalInput};
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};

use crate::authz::{Caller, Capability};
use crate::clock::now_rfc3339;
use crate::error::CoreError;
use crate::event::{Event, EventKind};
use crate::ids::{KernelId, SessionId};
use crate::session::{RecoveryState, Session, SessionSpec, SessionStatus};
use crate::store::{ContextStore, SessionRecord, SessionStore, StoreError};
use crate::terminal::{ScreenModel, TerminalGeometry};

/// How many output chunks a slow attached client may fall behind before it is
/// dropped from the live fan-out. It can always recover from a fresh snapshot,
/// so lagging is survivable; unbounded buffering is not.
pub(crate) const LIVE_BUFFER: usize = 512;

/// Builds a screen model at a given geometry.
///
/// Injected so the core never depends on a particular terminal parser.
pub type ScreenFactory = Arc<dyn Fn(TerminalGeometry) -> Box<dyn ScreenModel> + Send + Sync>;

/// An exclusive terminal attachment.
///
/// Holding this is what makes an attachment exclusive; dropping it releases the
/// lease. Attachment is not a tenth core operation — it is an exclusivity mode
/// on the live stream.
pub struct AttachmentHandle {
    session_id: SessionId,
    service: Arc<SessionService>,
    /// Live terminal output for as long as the attachment is held.
    pub output: broadcast::Receiver<Vec<u8>>,
    /// Bytes that repaint the current screen, delivered before live output.
    pub repaint: Vec<u8>,
    /// Geometry the repaint was rendered at.
    pub geometry: TerminalGeometry,
}

impl Drop for AttachmentHandle {
    fn drop(&mut self) {
        let service = Arc::clone(&self.service);
        let id = self.session_id.clone();
        // Release synchronously where possible; the lock is never held across
        // an await inside the service, so a blocking release cannot deadlock.
        tokio::spawn(async move {
            service.release_attachment(&id).await;
        });
    }
}

/// Everything about a session that exists only while its runtime does.
///
/// Deliberately not persisted: a screen model, a socket and an attachment lease
/// describe a live process, and a restarted one has none of them. They are
/// rebuilt when a client attaches.
pub(crate) struct SessionEntry {
    pub(crate) session: Session,
    /// Owner-delegated credential last handed to the runtime. Held so a refresh
    /// can be pushed without the caller tracking which sessions exist.
    pub(crate) subject_credential: Option<String>,
    pub(crate) events: Vec<Event>,
    pub(crate) screen: Arc<Mutex<Box<dyn ScreenModel>>>,
    pub(crate) runtime: Option<RuntimeHandle>,
    pub(crate) input: Option<mpsc::Sender<TerminalInput>>,
    /// Where this session's stored context lives, once any has been captured.
    pub(crate) context_ref: Option<String>,
    pub(crate) live: broadcast::Sender<Vec<u8>>,
    pub(crate) attached: bool,
}

/// The core service.
pub struct SessionService {
    /// Live sessions, keyed by id. Only those with a running runtime appear.
    pub(crate) entries: RwLock<HashMap<SessionId, SessionEntry>>,
    pub(crate) adapters: HashMap<KernelId, Arc<dyn KernelAdapter>>,
    pub(crate) screen_factory: ScreenFactory,
    /// The durable record. Source of truth for anything that outlives a process.
    pub(crate) store: Arc<dyn SessionStore>,
    /// Where an agent's accumulated context is kept, when a deployment has one.
    pub(crate) context: Option<Arc<dyn ContextStore>>,
    /// Milliseconds to wait before the first provisioning retry.
    ///
    /// Held here rather than as a constant so a test can exercise the retry
    /// path without waiting out a delay meant for a provider under load.
    pub(crate) retry_backoff_ms: std::sync::atomic::AtomicU64,
}

impl SessionService {
    /// Build a service over a set of runtime implementations.
    ///
    /// The map is the only place a concrete runtime is named; the core selects
    /// by opaque identifier and never interprets it.
    pub fn new(
        adapters: HashMap<KernelId, Arc<dyn KernelAdapter>>,
        screen_factory: ScreenFactory,
        store: Arc<dyn SessionStore>,
        context: Option<Arc<dyn ContextStore>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            entries: RwLock::new(HashMap::new()),
            adapters,
            screen_factory,
            store,
            context,
            retry_backoff_ms: std::sync::atomic::AtomicU64::new(
                crate::provision::DEFAULT_RETRY_BACKOFF_MS,
            ),
        })
    }

    /// Read a session's durable record.
    pub(crate) async fn record(&self, id: &SessionId) -> Result<SessionRecord, CoreError> {
        self.store
            .get(id)
            .await
            .map_err(store_failed)?
            .ok_or(CoreError::SessionNotFound)
    }

    /// Identifiers of the runtimes this service can back a session with.
    pub fn available_kernels(&self) -> Vec<KernelId> {
        let mut ids: Vec<_> = self.adapters.keys().cloned().collect();
        ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        ids
    }

    /// Create a session and bring its runtime up in the background.
    ///
    /// Returns as soon as the session exists, before any runtime does. Bringing
    /// a runtime up means pulling an image and scheduling a pod, which takes as
    /// long as it takes; holding a request open for it ties the work to a
    /// connection, and when that connection goes away — a timeout, a closed tab
    /// — the work is cancelled halfway, leaving a runtime nothing refers to.
    ///
    /// The record is written before anything is provisioned, so every runtime
    /// that comes to exist is already accounted for. Callers follow the rest
    /// through the event log.
    pub async fn create_session(
        self: &Arc<Self>,
        caller: &Caller,
        spec: SessionSpec,
    ) -> Result<Session, CoreError> {
        caller.authorization.require(Capability::CreateSession)?;
        // Resolved before the record is written: an unknown kernel is the
        // caller's mistake and should be refused outright rather than accepted
        // and then failed asynchronously.
        let adapter =
            self.adapters.get(&spec.kernel).cloned().ok_or_else(|| {
                CoreError::RuntimeUnavailable(format!("no runtime {}", spec.kernel))
            })?;

        let geometry = spec.geometry.unwrap_or_default();
        let id = SessionId::generate();
        let session = Session {
            id: id.clone(),
            owner: caller.subject.clone(),
            kernel: spec.kernel.clone(),
            status: SessionStatus::Provisioning,
            recovery_state: RecoveryState::Normal,
            stop_reason: None,
            geometry,
            last_seq: 0,
            created_at: now_rfc3339(),
        };

        let mut record = SessionRecord::new(session.clone());
        record.session.last_seq = 3;
        self.store.upsert(&record).await.map_err(store_failed)?;
        self.store
            .append_events(&[
                Event {
                    session_id: id.clone(),
                    seq: 1,
                    kind: EventKind::SessionCreated,
                },
                Event {
                    session_id: id.clone(),
                    seq: 2,
                    kind: EventKind::StatusChanged {
                        status: SessionStatus::Provisioning,
                        stop_reason: None,
                        detail: None,
                    },
                },
                Event {
                    session_id: id.clone(),
                    seq: 3,
                    kind: EventKind::Progress {
                        stage: crate::session::ProvisioningStage::Accepted,
                    },
                },
            ])
            .await
            .map_err(store_failed)?;

        // Detached deliberately. The caller's connection may end at any moment
        // and must not take the runtime with it.
        let service = Arc::clone(self);
        let spawned = record.session.clone();
        tokio::spawn(async move {
            service.bring_up(spawned, spec, adapter, geometry).await;
        });

        tracing::info!(session_id = %id, kernel = %record.session.kernel, "session created, runtime starting");
        Ok(record.session)
    }

    /// Read a session, scoped to its owner.
    pub async fn get_session(&self, caller: &Caller, id: &SessionId) -> Result<Session, CoreError> {
        let record = self.record(id).await?;
        if !caller.may_read_owned_by(&record.session.owner) {
            return Err(CoreError::Forbidden);
        }
        Ok(record.session)
    }

    /// List the subject's sessions, newest first.
    pub async fn list_sessions(&self, caller: &Caller) -> Vec<Session> {
        // A privileged reader sees every session; everyone else sees only their
        // own. The filter is here rather than in an interface so no interface
        // can forget it, and it is pushed into the query rather than applied
        // after so a large deployment does not read every record to show one.
        let result = if caller.authorization.allows(Capability::ReadAllSessions) {
            self.store.list_all().await
        } else {
            self.store.list_for(&caller.subject).await
        };
        match result {
            Ok(records) => records.into_iter().map(|r| r.session).collect(),
            Err(e) => {
                // Listing is a read: report nothing rather than pretend the
                // caller has no sessions, and make the failure visible.
                tracing::error!(error = %e, "cannot list sessions");
                Vec::new()
            }
        }
    }

    /// Durable events for a session, optionally from a cursor.
    pub async fn events_since(
        &self,
        caller: &Caller,
        id: &SessionId,
        after: Option<u64>,
    ) -> Result<Vec<Event>, CoreError> {
        let record = self.record(id).await?;
        if !caller.may_read_owned_by(&record.session.owner) {
            return Err(CoreError::Forbidden);
        }
        self.store
            .events_since(id, after.unwrap_or(0))
            .await
            .map_err(store_failed)
    }

    /// Take the exclusive terminal attachment.
    ///
    /// The geometry is applied to the screen before the repaint is produced, so
    /// the caller's first paint is already at its own size.
    pub async fn attach_terminal(
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
        geometry: TerminalGeometry,
    ) -> Result<AttachmentHandle, CoreError> {
        caller.authorization.require(Capability::UseOwnSessions)?;
        // A session whose process is gone still exists. Reviving it is the
        // caller's next step; refusing here would make a persisted session
        // unusable after the restart that persistence exists to survive.
        let record = self.record(id).await?;
        if !record.session.is_owned_by(&caller.subject) {
            return Err(CoreError::Forbidden);
        }
        if record.session.status.is_terminal() {
            return Err(CoreError::SessionTerminated);
        }
        if !self.entries.read().await.contains_key(id) {
            return Err(CoreError::RuntimeUnavailable(
                "session has no live runtime; revive it first".to_string(),
            ));
        }

        let (screen, live) = {
            let mut entries = self.entries.write().await;
            let entry = entries.get_mut(id).ok_or(CoreError::SessionNotFound)?;
            // Attaching is acting, not observing, so ownership is required
            // even for a privileged reader.
            if !entry.session.is_owned_by(&caller.subject) {
                return Err(CoreError::Forbidden);
            }
            if entry.session.status.is_terminal() {
                return Err(CoreError::SessionTerminated);
            }
            if entry.attached {
                return Err(CoreError::AlreadyAttached);
            }
            entry.attached = true;
            entry.session.geometry = geometry;
            append(entry, EventKind::TerminalAttached);
            (Arc::clone(&entry.screen), entry.live.clone())
        };
        self.flush(id).await;

        // Subscribe before rendering the repaint: output produced between the
        // two is then buffered rather than lost.
        let output = live.subscribe();

        let repaint = {
            let mut s = screen.lock().await;
            s.resize(geometry);
            s.repaint_bytes()
        };

        self.forward_input(
            id,
            TerminalInput::Resize(TermSize {
                cols: geometry.columns(),
                rows: geometry.rows(),
            }),
        )
        .await;

        tracing::info!(session_id = %id, cols = geometry.columns(), rows = geometry.rows(), "terminal attached");
        Ok(AttachmentHandle {
            session_id: id.clone(),
            service: Arc::clone(self),
            output,
            repaint,
            geometry,
        })
    }

    /// Release the exclusive attachment. Idempotent.
    pub async fn release_attachment(&self, id: &SessionId) {
        let mut entries = self.entries.write().await;
        if let Some(entry) = entries.get_mut(id) {
            if entry.attached {
                entry.attached = false;
                append(entry, EventKind::TerminalDetached);
                tracing::info!(session_id = %id, "terminal detached");
            }
        }
        drop(entries);
        self.flush(id).await;
    }

    /// Persist a session's current state and any events buffered against it.
    ///
    /// Called after every state change. Failures are logged rather than
    /// propagated where the caller has already acted: the runtime really did
    /// change, and losing the record is a storage problem to surface, not a
    /// reason to tell the caller their action failed.
    pub(crate) async fn flush(&self, id: &SessionId) {
        let (record, events) = {
            let mut entries = self.entries.write().await;
            let Some(entry) = entries.get_mut(id) else {
                return;
            };
            let events = std::mem::take(&mut entry.events);
            let mut record = SessionRecord::new(entry.session.clone());
            record.runtime_id = entry.runtime.as_ref().map(|h| h.0.clone());
            record.context_ref = entry.context_ref.clone();
            (record, events)
        };
        if let Err(e) = self.store.upsert(&record).await {
            tracing::error!(session_id = %id, error = %e, "cannot persist session");
        }
        if let Err(e) = self.store.append_events(&events).await {
            tracing::error!(session_id = %id, error = %e, "cannot persist events");
        }
    }

    /// Fail unless the caller owns this live session and it is still running.
    ///
    /// Scoped to live sessions on purpose: this guards the terminal-driving
    /// operations, which are meaningless without a runtime to drive.
    pub(crate) async fn authorize(&self, caller: &Caller, id: &SessionId) -> Result<(), CoreError> {
        let entries = self.entries.read().await;
        let entry = entries.get(id).ok_or(CoreError::SessionNotFound)?;
        if !entry.session.is_owned_by(&caller.subject) {
            return Err(CoreError::Forbidden);
        }
        if entry.session.status.is_terminal() {
            return Err(CoreError::SessionTerminated);
        }
        Ok(())
    }

    pub(crate) async fn forward_input(&self, id: &SessionId, input: TerminalInput) {
        let sender = {
            let entries = self.entries.read().await;
            entries.get(id).and_then(|e| e.input.clone())
        };
        if let Some(tx) = sender {
            if tx.send(input).await.is_err() {
                tracing::warn!(session_id = %id, "runtime input channel closed");
            }
        }
    }
}

/// Render a storage failure as a core error.
///
/// Storage being unreachable is reported as the runtime being unavailable
/// rather than as "not found": a session that exists but cannot be read is a
/// different situation from one that does not exist, and conflating them would
/// tell a client their work was gone when it is merely unreachable.
pub(crate) fn store_failed(e: StoreError) -> CoreError {
    CoreError::RuntimeUnavailable(format!("session store: {e}"))
}

/// Append a durable event, allocating the next per-session sequence.
///
/// Buffers on the entry; `flush` is what makes it durable. Split so a caller
/// holding the entries lock does not perform I/O while holding it.
pub(crate) fn append(entry: &mut SessionEntry, kind: EventKind) {
    entry.session.last_seq += 1;
    entry.events.push(Event {
        session_id: entry.session.id.clone(),
        seq: entry.session.last_seq,
        kind,
    });
}

/// Pump runtime output into the screen model and out to any live subscriber.
pub(crate) fn spawn_relay(
    id: SessionId,
    mut output: mpsc::Receiver<Vec<u8>>,
    screen: Arc<Mutex<Box<dyn ScreenModel>>>,
    live: broadcast::Sender<Vec<u8>>,
) {
    tokio::spawn(async move {
        while let Some(chunk) = output.recv().await {
            screen.lock().await.apply(&chunk);
            // A send with no subscribers is not an error: the session is simply
            // detached, and the screen model has already absorbed the bytes.
            let _ = live.send(chunk);
        }
        tracing::info!(session_id = %id, "runtime output ended");
    });
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
