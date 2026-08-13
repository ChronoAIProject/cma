//! The single tunnel: session lifecycle, the event log, and terminal relay.
//!
//! Every interface calls this and only this. The service owns one live screen
//! model per session and keeps it fed for as long as the runtime produces
//! output — whether or not a client is attached. A detached session that keeps
//! working is the normal case, and a client that reattaches must find the
//! screen already correct rather than replaying history.

use std::collections::HashMap;
use std::sync::Arc;

use layer3_adapter::{KernelAdapter, RuntimeHandle, RuntimeSpec, TermSize, TerminalInput};
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};

use crate::authz::{Caller, Capability};
use crate::clock::now_rfc3339;
use crate::error::CoreError;
use crate::event::{Event, EventKind};
use crate::ids::{KernelId, SessionId};
use crate::session::{RecoveryState, Session, SessionSpec, SessionStatus, StopReason};
use crate::terminal::{ScreenModel, ScreenSnapshot, TerminalGeometry};

/// How many output chunks a slow attached client may fall behind before it is
/// dropped from the live fan-out. It can always recover from a fresh snapshot,
/// so lagging is survivable; unbounded buffering is not.
const LIVE_BUFFER: usize = 512;

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

pub(crate) struct SessionEntry {
    pub(crate) session: Session,
    /// Owner-delegated credential last handed to the runtime. Held so a refresh
    /// can be pushed without the caller tracking which sessions exist.
    pub(crate) subject_credential: Option<String>,
    events: Vec<Event>,
    screen: Arc<Mutex<Box<dyn ScreenModel>>>,
    runtime: Option<RuntimeHandle>,
    pub(crate) input: Option<mpsc::Sender<TerminalInput>>,
    live: broadcast::Sender<Vec<u8>>,
    attached: bool,
}

/// The core service.
pub struct SessionService {
    pub(crate) entries: RwLock<HashMap<SessionId, SessionEntry>>,
    adapters: HashMap<KernelId, Arc<dyn KernelAdapter>>,
    screen_factory: ScreenFactory,
}

impl SessionService {
    /// Build a service over a set of runtime implementations.
    ///
    /// The map is the only place a concrete runtime is named; the core selects
    /// by opaque identifier and never interprets it.
    pub fn new(
        adapters: HashMap<KernelId, Arc<dyn KernelAdapter>>,
        screen_factory: ScreenFactory,
    ) -> Arc<Self> {
        Arc::new(Self {
            entries: RwLock::new(HashMap::new()),
            adapters,
            screen_factory,
        })
    }

    /// Identifiers of the runtimes this service can back a session with.
    pub fn available_kernels(&self) -> Vec<KernelId> {
        let mut ids: Vec<_> = self.adapters.keys().cloned().collect();
        ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        ids
    }

    /// Provision a session and start relaying its terminal.
    pub async fn create_session(
        self: &Arc<Self>,
        caller: &Caller,
        spec: SessionSpec,
    ) -> Result<Session, CoreError> {
        caller.authorization.require(Capability::CreateSession)?;
        let owner = caller.subject.clone();
        let adapter =
            self.adapters.get(&spec.kernel).cloned().ok_or_else(|| {
                CoreError::RuntimeUnavailable(format!("no runtime {}", spec.kernel))
            })?;

        let geometry = spec.geometry.unwrap_or_default();
        let id = SessionId::generate();

        let handle = adapter
            .provision(&RuntimeSpec {
                session_id: id.0.clone(),
                size: TermSize {
                    cols: geometry.columns(),
                    rows: geometry.rows(),
                },
                subject_credential: spec.subject_credential.clone(),
            })
            .await
            .map_err(|e| CoreError::RuntimeUnavailable(e.to_string()))?;

        let channel = adapter
            .attach_terminal(&handle)
            .await
            .map_err(|e| CoreError::RuntimeUnavailable(e.to_string()))?;

        let (live, _) = broadcast::channel(LIVE_BUFFER);
        let screen: Arc<Mutex<Box<dyn ScreenModel>>> =
            Arc::new(Mutex::new((self.screen_factory)(geometry)));

        let session = Session {
            id: id.clone(),
            owner,
            kernel: spec.kernel.clone(),
            status: SessionStatus::Ready,
            recovery_state: RecoveryState::Normal,
            stop_reason: None,
            geometry,
            last_seq: 0,
            created_at: now_rfc3339(),
        };

        let mut entry = SessionEntry {
            session: session.clone(),
            subject_credential: spec.subject_credential.clone(),
            events: Vec::new(),
            screen: Arc::clone(&screen),
            runtime: Some(handle),
            input: Some(channel.input.clone()),
            live: live.clone(),
            attached: false,
        };
        append(&mut entry, EventKind::SessionCreated);
        append(
            &mut entry,
            EventKind::StatusChanged {
                status: SessionStatus::Ready,
                stop_reason: None,
            },
        );
        let session = entry.session.clone();
        self.entries.write().await.insert(id.clone(), entry);

        // The relay runs for the session's life, independent of any client.
        // This is what makes a reattach cheap and a detached session correct.
        spawn_relay(id.clone(), channel.output, screen, live);

        // Hand over the owner's credential before the agent can need it, so the
        // first turn is already authenticated.
        if let Some(credential) = spec.subject_credential.clone() {
            let _ = channel
                .input
                .send(TerminalInput::Credential(credential))
                .await;
        }

        // Ask the runtime to match the requested geometry before anything is
        // painted, so the first frame is never rendered at the wrong size.
        let _ = channel
            .input
            .send(TerminalInput::Resize(TermSize {
                cols: geometry.columns(),
                rows: geometry.rows(),
            }))
            .await;

        tracing::info!(session_id = %id, kernel = %spec.kernel, "session created");
        Ok(session)
    }

    /// Read a session, scoped to its owner.
    pub async fn get_session(&self, caller: &Caller, id: &SessionId) -> Result<Session, CoreError> {
        let entries = self.entries.read().await;
        let entry = entries.get(id).ok_or(CoreError::SessionNotFound)?;
        if !caller.may_read_owned_by(&entry.session.owner) {
            return Err(CoreError::Forbidden);
        }
        Ok(entry.session.clone())
    }

    /// List the subject's sessions, newest first.
    pub async fn list_sessions(&self, caller: &Caller) -> Vec<Session> {
        // A privileged reader sees every session; everyone else sees only their
        // own. The filter is here rather than in an interface so no interface
        // can forget it.
        let all = caller.authorization.allows(Capability::ReadAllSessions);
        let entries = self.entries.read().await;
        let mut out: Vec<_> = entries
            .values()
            .filter(|e| all || e.session.is_owned_by(&caller.subject))
            .map(|e| e.session.clone())
            .collect();
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        out
    }

    /// Durable events for a session, optionally from a cursor.
    pub async fn events_since(
        &self,
        caller: &Caller,
        id: &SessionId,
        after: Option<u64>,
    ) -> Result<Vec<Event>, CoreError> {
        let entries = self.entries.read().await;
        let entry = entries.get(id).ok_or(CoreError::SessionNotFound)?;
        if !caller.may_read_owned_by(&entry.session.owner) {
            return Err(CoreError::Forbidden);
        }
        let floor = after.unwrap_or(0);
        Ok(entry
            .events
            .iter()
            .filter(|e| e.seq > floor)
            .cloned()
            .collect())
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
    }

    /// Deliver input to the session's terminal.
    pub async fn send_input(
        &self,
        caller: &Caller,
        id: &SessionId,
        bytes: Vec<u8>,
    ) -> Result<(), CoreError> {
        self.authorize(caller, id).await?;
        self.forward_input(id, TerminalInput::Bytes(bytes)).await;
        Ok(())
    }

    /// Send an out-of-band signal to the session's runtime.
    pub async fn send_signal(
        &self,
        caller: &Caller,
        id: &SessionId,
        signal: String,
    ) -> Result<(), CoreError> {
        self.authorize(caller, id).await?;
        self.forward_input(id, TerminalInput::Signal(signal)).await;
        Ok(())
    }

    /// Change the session's terminal geometry.
    pub async fn resize_terminal(
        &self,
        caller: &Caller,
        id: &SessionId,
        geometry: TerminalGeometry,
    ) -> Result<(), CoreError> {
        let screen = {
            let mut entries = self.entries.write().await;
            let entry = entries.get_mut(id).ok_or(CoreError::SessionNotFound)?;
            if !entry.session.is_owned_by(&caller.subject) {
                return Err(CoreError::Forbidden);
            }
            entry.session.geometry = geometry;
            append(
                entry,
                EventKind::TerminalResized {
                    columns: geometry.columns(),
                    rows: geometry.rows(),
                },
            );
            Arc::clone(&entry.screen)
        };
        screen.lock().await.resize(geometry);
        self.forward_input(
            id,
            TerminalInput::Resize(TermSize {
                cols: geometry.columns(),
                rows: geometry.rows(),
            }),
        )
        .await;
        Ok(())
    }

    /// Read the current screen without holding a stream.
    pub async fn screen_snapshot(
        &self,
        caller: &Caller,
        id: &SessionId,
    ) -> Result<ScreenSnapshot, CoreError> {
        let screen = {
            let entries = self.entries.read().await;
            let entry = entries.get(id).ok_or(CoreError::SessionNotFound)?;
            if !caller.may_read_owned_by(&entry.session.owner) {
                return Err(CoreError::Forbidden);
            }
            Arc::clone(&entry.screen)
        };
        let snapshot = screen.lock().await.snapshot();
        Ok(snapshot)
    }

    /// End a session and tear its runtime down.
    pub async fn terminate_session(
        &self,
        caller: &Caller,
        id: &SessionId,
    ) -> Result<(), CoreError> {
        caller
            .authorization
            .require(Capability::TerminateOwnSession)?;
        let (adapter, runtime) = {
            let mut entries = self.entries.write().await;
            let entry = entries.get_mut(id).ok_or(CoreError::SessionNotFound)?;
            if !entry.session.is_owned_by(&caller.subject) {
                return Err(CoreError::Forbidden);
            }
            entry.session.status = SessionStatus::Terminated;
            entry.session.stop_reason = Some(StopReason::UserTerminated);
            entry.session.recovery_state = RecoveryState::Retired;
            append(
                entry,
                EventKind::StatusChanged {
                    status: SessionStatus::Terminated,
                    stop_reason: Some(StopReason::UserTerminated),
                },
            );
            entry.input = None;
            (
                self.adapters.get(&entry.session.kernel).cloned(),
                entry.runtime.clone(),
            )
        };

        if let (Some(adapter), Some(handle)) = (adapter, runtime) {
            if let Err(e) = adapter.terminate(&handle).await {
                // The session is already marked terminated; a runtime that
                // refuses teardown is logged, never silently swallowed.
                tracing::warn!(session_id = %id, error = %e, "runtime teardown failed");
            }
        }
        tracing::info!(session_id = %id, "session terminated");
        Ok(())
    }

    async fn authorize(&self, caller: &Caller, id: &SessionId) -> Result<(), CoreError> {
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

    async fn forward_input(&self, id: &SessionId, input: TerminalInput) {
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

/// Append a durable event, allocating the next per-session sequence.
fn append(entry: &mut SessionEntry, kind: EventKind) {
    entry.session.last_seq += 1;
    entry.events.push(Event {
        session_id: entry.session.id.clone(),
        seq: entry.session.last_seq,
        kind,
    });
}

/// Pump runtime output into the screen model and out to any live subscriber.
fn spawn_relay(
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
