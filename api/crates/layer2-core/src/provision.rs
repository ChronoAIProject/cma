//! Bringing a session's runtime up, after the session already exists.
//!
//! This runs detached from whoever asked for the session, because provisioning
//! takes as long as pulling an image and scheduling a pod takes, and tying that
//! to a request means a closed connection cancels it halfway.
//!
//! Detaching it introduces the failure this module is mostly about. Nobody is
//! waiting to be told it went wrong, so a failure that is merely logged leaves
//! a session sitting in `provisioning` forever — which is worse than the
//! synchronous version, where at least the caller saw the error. Every path out
//! of here therefore ends in a recorded state.

use std::sync::Arc;
use std::time::Duration;

use layer3_adapter::{
    KernelAdapter, RuntimeHandle, RuntimeOptions, RuntimePermission, RuntimeSpec, TermSize,
    TerminalInput,
};
use tokio::sync::{broadcast, Mutex};

use crate::event::EventKind;
use crate::ids::SessionId;
use crate::options::{AgentOptions, PermissionMode};
use crate::service::{spawn_relay, SessionEntry, SessionService, LIVE_BUFFER};
use crate::session::{RecoveryState, Session, SessionSpec, SessionStatus, StopReason};
use crate::terminal::{ScreenModel, TerminalGeometry};

/// How long a runtime may take to become usable.
///
/// Generous, because a cold image pull onto a fresh node is not fast and giving
/// up early would fail sessions that were about to work. Bounded all the same:
/// without a deadline a provider that never answers leaves a session waiting
/// for something that is not coming, and the sandbox it may already have
/// created is never reclaimed.
const PROVISION_DEADLINE: Duration = Duration::from_secs(300);

impl SessionService {
    /// Provision a runtime for an existing session and attach its terminal.
    pub(crate) async fn bring_up(
        self: &Arc<Self>,
        session: Session,
        spec: SessionSpec,
        adapter: Arc<dyn KernelAdapter>,
        geometry: TerminalGeometry,
    ) {
        let id = session.id.clone();
        match tokio::time::timeout(
            PROVISION_DEADLINE,
            self.start_runtime(&session, &spec, &adapter, geometry),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(reason)) => self.abandon(&id, &adapter, &reason).await,
            Err(_) => {
                self.abandon(
                    &id,
                    &adapter,
                    &format!(
                        "the runtime did not become usable within {}s",
                        PROVISION_DEADLINE.as_secs()
                    ),
                )
                .await
            }
        }
    }

    /// The successful path, from nothing to an attached terminal.
    async fn start_runtime(
        self: &Arc<Self>,
        session: &Session,
        spec: &SessionSpec,
        adapter: &Arc<dyn KernelAdapter>,
        geometry: TerminalGeometry,
    ) -> Result<(), String> {
        let id = session.id.clone();
        let handle = adapter
            .provision(&RuntimeSpec {
                session_id: id.0.clone(),
                size: TermSize {
                    cols: geometry.columns(),
                    rows: geometry.rows(),
                },
                subject_credential: spec.subject_credential.clone(),
                options: runtime_options(&spec.options),
            })
            .await
            .map_err(|e| format!("could not provision a runtime: {e}"))?;

        // Recorded before anything else is attempted. From here a runtime
        // exists, and a record that does not name it is one nothing can reclaim.
        self.remember_runtime(&id, &handle).await;

        let channel = adapter
            .attach_terminal(&handle)
            .await
            .map_err(|e| format!("could not attach to the runtime: {e}"))?;

        let (live, _) = broadcast::channel(LIVE_BUFFER);
        let screen: Arc<Mutex<Box<dyn ScreenModel>>> =
            Arc::new(Mutex::new((self.screen_factory)(geometry)));

        let mut entry = SessionEntry {
            session: session.clone(),
            subject_credential: spec.subject_credential.clone(),
            events: Vec::new(),
            screen: Arc::clone(&screen),
            runtime: Some(handle),
            input: Some(channel.input.clone()),
            live: live.clone(),
            attached: false,
            context_ref: None,
        };
        entry.session.status = SessionStatus::Ready;
        crate::service::append(
            &mut entry,
            EventKind::StatusChanged {
                status: SessionStatus::Ready,
                stop_reason: None,
            },
        );
        self.entries.write().await.insert(id.clone(), entry);

        // The relay runs for the session's life, independent of any client.
        spawn_relay(id.clone(), channel.output, screen, live);

        // Hand over the owner's credential before the agent can need it, so the
        // first turn is already authenticated.
        if let Some(credential) = spec.subject_credential.clone() {
            let _ = channel
                .input
                .send(TerminalInput::Credential(credential))
                .await;
        }
        let _ = channel
            .input
            .send(TerminalInput::Resize(TermSize {
                cols: geometry.columns(),
                rows: geometry.rows(),
            }))
            .await;

        self.flush(&id).await;
        tracing::info!(session_id = %id, "runtime ready");
        Ok(())
    }

    /// Record the runtime a session was given, as soon as it exists.
    async fn remember_runtime(&self, id: &SessionId, handle: &RuntimeHandle) {
        let Ok(Some(mut record)) = self.store.get(id).await else {
            tracing::error!(session_id = %id, "cannot record the runtime: session is gone");
            return;
        };
        record.runtime_id = Some(handle.0.clone());
        if let Err(e) = self.store.upsert(&record).await {
            // Worth shouting about: the runtime exists and the record does not
            // name it, which is exactly the orphan reconciliation must catch.
            tracing::error!(session_id = %id, error = %e, "runtime provisioned but not recorded");
        }
    }

    /// Give up on a session, saying why, and take any runtime down with it.
    ///
    /// The state is written even though nobody is waiting for it. That is the
    /// whole point: the caller left long ago, and a session that fails silently
    /// is indistinguishable from one still starting.
    async fn abandon(&self, id: &SessionId, adapter: &Arc<dyn KernelAdapter>, reason: &str) {
        tracing::error!(session_id = %id, reason, "session could not be started");

        let runtime = match self.store.get(id).await {
            Ok(Some(mut record)) => {
                let handle = record.runtime_id.take().map(RuntimeHandle);
                record.session.status = SessionStatus::Terminated;
                record.session.stop_reason = Some(StopReason::Error);
                record.session.recovery_state = RecoveryState::Retired;
                record.session.last_seq += 1;
                let seq = record.session.last_seq;
                let session_id = record.session.id.clone();
                if let Err(e) = self.store.upsert(&record).await {
                    tracing::error!(session_id = %id, error = %e, "cannot record the failure");
                }
                if let Err(e) = self
                    .store
                    .append_events(&[crate::event::Event {
                        session_id,
                        seq,
                        kind: EventKind::StatusChanged {
                            status: SessionStatus::Terminated,
                            stop_reason: Some(StopReason::Error),
                        },
                    }])
                    .await
                {
                    tracing::error!(session_id = %id, error = %e, "cannot record the failure event");
                }
                handle
            }
            _ => None,
        };

        self.entries.write().await.remove(id);

        // A runtime may exist even though the session failed — provisioning can
        // succeed and attaching still fail. Leaving it running would be the
        // leak this design exists to prevent.
        if let Some(handle) = runtime {
            if let Err(e) = adapter.terminate(&handle).await {
                tracing::warn!(session_id = %id, error = %e, "could not tear down the runtime");
            }
        }
    }
}

/// Translate what a person asked for into what a runtime is told.
///
/// The two vocabularies are separate on purpose — the core must not speak any
/// runtime's dialect — so this is the single place they meet, and the only
/// place a new option has to be taught to cross.
fn runtime_options(options: &AgentOptions) -> RuntimeOptions {
    RuntimeOptions {
        model: options.model.clone(),
        reasoning_effort: Some(options.reasoning_effort.as_str().to_string()),
        permission: match options.permission {
            PermissionMode::Guarded => RuntimePermission::Guarded,
            PermissionMode::Autonomous => RuntimePermission::Autonomous,
            PermissionMode::Unrestricted => RuntimePermission::Unrestricted,
        },
        own_credential: matches!(options.credential, crate::options::CredentialMode::Own),
    }
}
