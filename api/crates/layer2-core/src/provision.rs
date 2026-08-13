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

use crate::authz::{Caller, Capability};
use crate::error::CoreError;
use crate::event::{Event, EventKind};
use crate::ids::SessionId;
use crate::options::{AgentOptions, PermissionMode};
use crate::service::{spawn_relay, store_failed, SessionEntry, SessionService, LIVE_BUFFER};
use crate::session::{
    ProvisioningStage, RecoveryState, Session, SessionSpec, SessionStatus, StopReason,
};
use crate::store::SessionRecord;
use crate::terminal::{ScreenModel, TerminalGeometry};

/// How long a runtime may take to become usable.
///
/// Generous, because a cold image pull onto a fresh node is not fast and giving
/// up early would fail sessions that were about to work. Bounded all the same:
/// without a deadline a provider that never answers leaves a session waiting
/// for something that is not coming, and the sandbox it may already have
/// created is never reclaimed.
const PROVISION_DEADLINE: Duration = Duration::from_secs(600);

/// How many times to try bringing a runtime up before giving up.
///
/// Provisioning fails for reasons that are nobody's decision and usually pass:
/// a provider under load, a node being replaced, a pull that took longer than
/// the call allowed. Someone asking for a session wants a session, and a
/// transient failure is not an answer to give them — so the first failure is
/// retried rather than reported.
const PROVISION_ATTEMPTS: u32 = 4;

/// Wait before the first retry, doubling thereafter.
///
/// Long enough to let a provider under load recover, short enough that a
/// session which succeeds on the second attempt still feels like it started
/// rather than like it stalled.
pub(crate) const DEFAULT_RETRY_BACKOFF_MS: u64 = 3_000;

impl SessionService {
    /// Provision a runtime for an existing session and attach its terminal.
    pub(crate) async fn bring_up(
        self: &Arc<Self>,
        session: Session,
        spec: SessionSpec,
        adapter: Arc<dyn KernelAdapter>,
        geometry: TerminalGeometry,
        restore: Option<Vec<u8>>,
    ) {
        let id = session.id.clone();
        // Nobody may have been present to delegate one — a session recovered by
        // the controller has no request behind it — so it is fetched here
        // rather than being required of every caller.
        let mut spec = spec;
        if spec.subject_credential.is_none() {
            spec.subject_credential = self.credential_for(&session.owner).await;
        }
        match tokio::time::timeout(
            PROVISION_DEADLINE,
            self.start_with_retries(&session, &spec, &adapter, geometry, restore.as_ref()),
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
        // Released last, and on every path: while the claim is held the
        // controller leaves this session alone, so failing to release it would
        // strand a session nothing is allowed to recover.
        self.finish_start(&id).await;
    }

    /// Keep trying to bring a runtime up until one does, or the attempts run out.
    ///
    /// Each attempt starts clean. A previous attempt may have created a runtime
    /// before failing — an attach that timed out leaves one running — and
    /// retrying without tearing that down would leak one sandbox per attempt.
    async fn start_with_retries(
        self: &Arc<Self>,
        session: &Session,
        spec: &SessionSpec,
        adapter: &Arc<dyn KernelAdapter>,
        geometry: TerminalGeometry,
        restore: Option<&Vec<u8>>,
    ) -> Result<(), String> {
        let mut backoff = Duration::from_millis(
            self.retry_backoff_ms
                .load(std::sync::atomic::Ordering::Relaxed),
        );
        let mut last = String::new();

        for attempt in 1..=PROVISION_ATTEMPTS {
            match self
                .start_runtime(session, spec, adapter, geometry, restore)
                .await
            {
                Ok(()) => {
                    if attempt > 1 {
                        tracing::info!(
                            session_id = %session.id,
                            attempt,
                            "runtime started after retrying"
                        );
                    }
                    return Ok(());
                }
                Err(reason) => {
                    tracing::warn!(
                        session_id = %session.id,
                        attempt,
                        of = PROVISION_ATTEMPTS,
                        reason = %reason,
                        "could not start the runtime; trying again"
                    );
                    self.discard_runtime(&session.id, adapter).await;
                    last = reason;
                }
            }
            if attempt < PROVISION_ATTEMPTS {
                tokio::time::sleep(backoff).await;
                backoff *= 2;
            }
        }

        Err(format!(
            "could not start a runtime after {PROVISION_ATTEMPTS} attempts. Last error: {last}"
        ))
    }

    /// Tear down whatever a failed attempt left behind.
    ///
    /// Without this each retry strands the runtime the previous one created,
    /// which is how a session that eventually succeeds still costs several
    /// sandboxes.
    async fn discard_runtime(&self, id: &SessionId, adapter: &Arc<dyn KernelAdapter>) {
        let Ok(Some(mut record)) = self.store.get(id).await else {
            return;
        };
        let Some(handle) = record.runtime_id.take().map(RuntimeHandle) else {
            return;
        };
        if let Err(e) = adapter.terminate(&handle).await {
            tracing::warn!(session_id = %id, error = %e, "could not discard a failed runtime");
        }
        if let Err(e) = self.store.upsert(&record).await {
            tracing::error!(session_id = %id, error = %e, "cannot clear the discarded runtime");
        }
        self.entries.write().await.remove(id);
    }

    /// The successful path, from nothing to an attached terminal.
    async fn start_runtime(
        self: &Arc<Self>,
        session: &Session,
        spec: &SessionSpec,
        adapter: &Arc<dyn KernelAdapter>,
        geometry: TerminalGeometry,
        restore: Option<&Vec<u8>>,
    ) -> Result<(), String> {
        let id = session.id.clone();
        self.note_progress(&id, ProvisioningStage::RuntimeRequested)
            .await;
        let handle = adapter
            .provision(&RuntimeSpec {
                session_id: id.0.clone(),
                size: TermSize {
                    cols: geometry.columns(),
                    rows: geometry.rows(),
                },
                subject_credential: spec.subject_credential.clone(),
                options: runtime_options(&spec.options),
                restore_from: restore.cloned(),
            })
            .await
            .map_err(|e| format!("could not provision a runtime: {e}"))?;

        // Recorded before anything else is attempted. From here a runtime
        // exists, and a record that does not name it is one nothing can reclaim.
        self.remember_runtime(&id, &handle).await;
        self.note_progress(&id, ProvisioningStage::RuntimeCreated)
            .await;
        self.note_progress(&id, ProvisioningStage::Attaching).await;

        let channel = adapter
            .attach_terminal(&handle)
            .await
            .map_err(|e| format!("could not attach to the runtime: {e}"))?;

        let (live, _) = broadcast::channel(LIVE_BUFFER);
        let screen: Arc<Mutex<Box<dyn ScreenModel>>> =
            Arc::new(Mutex::new((self.screen_factory)(geometry)));

        // Seeded from the record rather than from the copy passed in, because
        // the record is the authority and the copy is a snapshot from before
        // this task started. It has moved on since: progress events have been
        // appended against it, and a session being carried between runtimes
        // has had its version and rotation count written. Anything not read
        // back here is silently reverted by the next flush.
        let (mut seeded, context_ref) = match self.store.get(&id).await {
            Ok(Some(record)) => (record.session, record.context_ref),
            _ => (session.clone(), None),
        };
        seeded.geometry = geometry;

        let mut entry = SessionEntry {
            session: seeded,
            subject_credential: spec.subject_credential.clone(),
            events: Vec::new(),
            screen: Arc::clone(&screen),
            runtime: Some(handle),
            input: Some(channel.input.clone()),
            live: live.clone(),
            attached: false,
            context_ref,
            captured_len: 0,
        };
        entry.session.status = SessionStatus::Ready;
        crate::service::append(
            &mut entry,
            EventKind::StatusChanged {
                status: SessionStatus::Ready,
                stop_reason: None,
                detail: None,
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
        self.note_progress(&id, ProvisioningStage::Ready).await;
        tracing::info!(session_id = %id, "runtime ready");
        Ok(())
    }

    /// Append a progress step to the session's log.
    ///
    /// Written durably rather than pushed at whoever happens to be watching,
    /// so someone who opens the page mid-start still sees where it got to. A
    /// step that cannot be recorded is logged and dropped: progress is
    /// commentary, and failing a session because its commentary failed would
    /// be absurd.
    async fn note_progress(&self, id: &SessionId, stage: ProvisioningStage) {
        let Ok(Some(mut record)) = self.store.get(id).await else {
            return;
        };
        record.session.last_seq += 1;
        let seq = record.session.last_seq;
        if let Err(e) = self.store.upsert(&record).await {
            tracing::warn!(session_id = %id, error = %e, "cannot record progress");
            return;
        }
        if let Err(e) = self
            .store
            .append_events(&[Event {
                session_id: id.clone(),
                seq,
                kind: EventKind::Progress { stage },
            }])
            .await
        {
            tracing::warn!(session_id = %id, error = %e, "cannot append progress");
        }
    }

    /// Record the runtime a session was given, as soon as it exists.
    async fn remember_runtime(&self, id: &SessionId, handle: &RuntimeHandle) {
        let Ok(Some(mut record)) = self.store.get(id).await else {
            tracing::error!(session_id = %id, "cannot record the runtime: session is gone");
            return;
        };
        record.runtime_id = Some(handle.0.clone());
        // Stamped with what it was actually started on, so the next sweep can
        // tell a session that is current from one that is behind. Without this
        // every session would look due for rotation forever.
        record.session.runtime_version = self.runtime_version.clone();
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
                            // The reason travels with the event so the person
                            // watching learns what the log already knew.
                            detail: Some(reason.to_string()),
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

impl SessionService {
    /// Give a stopped session a runtime again.
    ///
    /// The same machinery that starts a session, pointed at one that already
    /// exists: the identity, the history and the options are unchanged, and
    /// only the sandbox is new. Reviving is therefore never a different kind of
    /// session, which matters because everything referring to it keeps working.
    pub async fn revive_session(
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
    ) -> Result<Session, CoreError> {
        caller.authorization.require(Capability::CreateSession)?;
        let record = self.record(id).await?;
        if !record.session.is_owned_by(&caller.subject) {
            return Err(CoreError::Forbidden);
        }
        if record.session.status.is_terminal() {
            return Err(CoreError::SessionTerminated);
        }
        if record.session.status.wants_runtime() {
            // Already running or already starting. Reviving again would
            // provision a second sandbox for one session.
            return Ok(record.session);
        }

        let adapter = self
            .adapters
            .get(&record.session.kernel)
            .cloned()
            .ok_or_else(|| {
                CoreError::RuntimeUnavailable(format!("no runtime {}", record.session.kernel))
            })?;

        let mut starting = record.clone();
        starting.session.status = SessionStatus::Provisioning;
        starting.session.stop_reason = None;
        starting.session.last_seq += 1;
        let seq = starting.session.last_seq;
        self.store.upsert(&starting).await.map_err(store_failed)?;
        self.store
            .append_events(&[Event {
                session_id: id.clone(),
                seq,
                kind: EventKind::StatusChanged {
                    status: SessionStatus::Provisioning,
                    stop_reason: None,
                    detail: None,
                },
            }])
            .await
            .map_err(store_failed)?;

        let geometry = starting.session.geometry;
        let spec = SessionSpec {
            kernel: starting.session.kernel.clone(),
            geometry: Some(geometry),
            options: starting.session.options.clone(),
            // The owner is not on the line to delegate one; the renewal loop
            // hands the session a fresh credential once it is running.
            subject_credential: None,
        };

        self.begin_start(id).await;
        let service = Arc::clone(self);
        let session = starting.session.clone();
        let spawned = session.clone();
        tokio::spawn(async move {
            service
                .bring_up(spawned, spec, adapter, geometry, None)
                .await;
        });

        tracing::info!(session_id = %id, "session reviving");
        Ok(session)
    }

    /// Record that a session which was starting is now usable.
    async fn settle_ready(&self, record: &SessionRecord) {
        let mut settled = record.clone();
        settled.session.status = SessionStatus::Ready;
        settled.session.stop_reason = None;
        settled.session.last_seq += 1;
        let seq = settled.session.last_seq;
        let id = settled.session.id.clone();
        if let Err(e) = self.store.upsert(&settled).await {
            tracing::warn!(session_id = %id, error = %e, "cannot record the recovered status");
            return;
        }
        if let Err(e) = self
            .store
            .append_events(&[Event {
                session_id: id.clone(),
                seq,
                kind: EventKind::StatusChanged {
                    status: SessionStatus::Ready,
                    stop_reason: None,
                    detail: None,
                },
            }])
            .await
        {
            tracing::warn!(session_id = %id, error = %e, "cannot append the recovered status");
        }
        if let Some(entry) = self.entries.write().await.get_mut(&id) {
            entry.session.status = SessionStatus::Ready;
            entry.session.last_seq = seq;
        }
    }

    /// Take an existing runtime back under management after losing track of it.
    ///
    /// A sandbox belongs to the provider, not to this process, so it is still
    /// there after a restart while the attachment that was driving it is not.
    /// Rebuilding that attachment is the difference between a deployment being
    /// invisible and every open session becoming unusable while still claiming
    /// to be ready.
    ///
    /// The screen is rebuilt from the stored transcript, so what someone left
    /// is what they come back to. Without that the session would be alive and
    /// usable but visibly empty, which reads as lost work.
    pub(crate) async fn readopt(&self, record: &SessionRecord) -> Result<(), CoreError> {
        let id = record.session.id.clone();
        let Some(handle) = record.runtime_id.clone().map(RuntimeHandle) else {
            // Nothing to reclaim: the session never got a runtime, or its
            // runtime was already torn down.
            return Err(CoreError::RuntimeUnavailable(
                "this session has no runtime to reconnect to".to_string(),
            ));
        };
        let Some(adapter) = self.adapters.get(&record.session.kernel).cloned() else {
            return Err(CoreError::RuntimeUnavailable(format!(
                "no runtime {}",
                record.session.kernel
            )));
        };

        let channel = adapter.attach_terminal(&handle).await.map_err(|e| {
            // The sandbox is genuinely gone. Say so in terms of what the person
            // can do rather than repeating the provider's wording.
            tracing::warn!(session_id = %id, error = %e, "cannot reconnect to the runtime");
            CoreError::RuntimeUnavailable(
                "the sandbox for this session is no longer running".to_string(),
            )
        })?;

        let (live, _) = broadcast::channel(LIVE_BUFFER);
        let mut model = (self.screen_factory)(record.session.geometry);

        // Replayed before anything new arrives, so the person reattaching sees
        // the conversation they left rather than a blank screen above their
        // prompt. Feeding the stored bytes through the model rather than
        // holding them aside rebuilds its parser state too, which is what makes
        // the next chunk of output land in the right place.
        let restored = self
            .stored_transcript(record.context_ref.as_deref())
            .await
            .unwrap_or_default();
        if !restored.is_empty() {
            model.apply(&restored);
            tracing::info!(session_id = %id, bytes = restored.len(), "transcript restored");
        }
        let screen: Arc<Mutex<Box<dyn ScreenModel>>> = Arc::new(Mutex::new(model));
        let captured_len = screen.lock().await.repaint_bytes().len();

        let entry = SessionEntry {
            session: record.session.clone(),
            subject_credential: None,
            events: Vec::new(),
            screen: Arc::clone(&screen),
            runtime: Some(handle),
            input: Some(channel.input.clone()),
            live: live.clone(),
            attached: false,
            context_ref: record.context_ref.clone(),
            captured_len,
        };
        self.entries.write().await.insert(id.clone(), entry);
        spawn_relay(id.clone(), channel.output, screen, live);

        // The sandbox kept running, but the credential it was given belonged to
        // a process that has gone. Without this the session answers every
        // prompt with an authorisation failure.
        self.supply_credential(&id, &record.session.owner).await;

        // A session recorded as still starting whose sandbox answers is not
        // starting — it started, and the process that would have said so died.
        // Leaving it as `provisioning` would show a working session as one
        // stuck part-way.
        if record.session.status == SessionStatus::Provisioning {
            self.settle_ready(record).await;
        }

        tracing::info!(session_id = %id, "reconnected to an existing runtime");
        Ok(())
    }
}
