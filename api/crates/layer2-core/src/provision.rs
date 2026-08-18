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

use crate::capabilities::SessionCapabilities;
use crate::event::{Event, EventKind};
use crate::ids::SessionId;
use crate::options::{AgentOptions, PermissionMode};
use crate::provision_failure::StartFailure;
use crate::service::{spawn_relay, SessionEntry, SessionService, LIVE_BUFFER};
use crate::session::{ProvisioningStage, Session, SessionSpec, SessionStatus};
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
pub(crate) const PROVISION_ATTEMPTS: u32 = 4;

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
            let services = self
                .environments
                .services_for(&session.owner, session.environment_profile.as_ref())
                .await;
            spec.subject_credential = self
                .credential_for(&session.id, &session.owner, &services)
                .await;
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

    /// The successful path, from nothing to an attached terminal.
    pub(crate) async fn start_runtime(
        self: &Arc<Self>,
        session: &Session,
        spec: &SessionSpec,
        adapter: &Arc<dyn KernelAdapter>,
        geometry: TerminalGeometry,
        restore: Option<&Vec<u8>>,
    ) -> Result<(), StartFailure> {
        let id = session.id.clone();
        // Checked again, because this also runs for a session being revived or
        // carried onto a replacement runtime — long after it was created, and
        // possibly onto a build of its kernel that no longer does what the
        // session was granted. Starting it anyway would leave a session whose
        // recorded abilities and real ones disagree, which is the state every
        // caller downstream trusts cannot happen.
        //
        // Settled rather than merely checked: a session recorded before this
        // model existed holds nothing, and nothing is a set that refuses every
        // operation it is consulted for. The record is read back below, so
        // writing it here is what the entry is then built from.
        self.settle_capabilities(session, adapter)
            .await
            .map_err(|refusal| {
                StartFailure::permanent(format!(
                    "this session's runtime can no longer be what it is: {refusal}"
                ))
            })?;
        // Resolved from the session rather than the request, so a runtime
        // replaced long afterwards is built the same way. A profile that has
        // since been deleted or changed hands fails the attempt rather than
        // silently starting a session without the tools it is meant to have.
        let environment = self
            .environments
            .resolve_for_owner(&session.owner, session.environment_profile.as_ref())
            .await
            .map_err(|e| {
                StartFailure::transient(format!(
                    "this session's environment could not be prepared: {e}"
                ))
            })?;
        self.note_progress(&id, ProvisioningStage::RuntimeRequested)
            .await;
        // What the runtime is told, translated once. Some kernels settle how
        // much the agent may do unattended per conversation rather than per
        // runtime, so this is needed twice and computing it twice would be two
        // places for the translation to drift.
        let options = runtime_options(&spec.options);
        /*
         * Read from the session rather than from the profile, exactly as the
         * credential's own scope is: a session keeps what it was granted when
         * it was provisioned, and a bundle widened or narrowed since must not
         * change what a running session believes it may reach.
         *
         * Names only. The credential minted against them travels a different
         * path entirely and never enters the sandbox.
         */
        let reaches: Vec<String> = self
            .services_for(&id)
            .await
            .into_iter()
            .map(|service| service.0)
            .collect();
        let handle = adapter
            .provision(&RuntimeSpec {
                session_id: id.0.clone(),
                size: TermSize {
                    cols: geometry.columns(),
                    rows: geometry.rows(),
                },
                subject_credential: spec.subject_credential.clone(),
                options: options.clone(),
                restore_from: restore.cloned(),
                reaches,
                environment: environment.into(),
                created_by: self.identity.clone(),
            })
            .await
            .map_err(StartFailure::from_provisioning)?;

        // Recorded before anything else is attempted. From here a runtime
        // exists, and a record that does not name it is one nothing can reclaim.
        self.remember_runtime(&id, &handle).await;
        self.note_progress(&id, ProvisioningStage::RuntimeCreated)
            .await;
        self.note_progress(&id, ProvisioningStage::Attaching).await;

        // Whichever plane this session was granted, and only that one. A
        // terminal session opens no conversation and a conversation session
        // attaches no terminal: the two are exclusive by construction, and
        // opening both would put a kernel on two planes for one session.
        let conversing = session.capabilities.conversation;
        let terminal = match conversing {
            false => Some(adapter.attach_terminal(&handle).await.map_err(|e| {
                StartFailure::transient(format!("could not attach to the runtime: {e}"))
            })?),
            true => None,
        };
        let conversation = match conversing {
            true => Some(
                adapter
                    .open_conversation(&handle, &options)
                    .await
                    .map_err(|e| {
                        // A kernel that advertised a conversation and cannot open one
                        // will not start being able to. Refused permanently so the
                        // owner is told now rather than after the backoff.
                        StartFailure::permanent(format!("could not open the conversation: {e}"))
                    })?,
            ),
            false => None,
        };

        let named_conversation = conversation
            .as_ref()
            .and_then(|c| c.conversation_id.clone());

        let (live, _) = broadcast::channel(LIVE_BUFFER);
        let (live_frames, _) = broadcast::channel(LIVE_BUFFER);

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

        // Built for either plane, and fed only by the terminal. A conversation
        // session never writes to it — the honest shape would be one entry per
        // plane, which is a larger change than this and is worth making once
        // rather than halfway.
        let mut model = (self.screen_factory)(geometry);
        /*
         * Replayed into the new screen, exactly as readopt does. This path
         * serves a session being revived or given a replacement runtime, and
         * both are the same session: §5 asks that everything it has shown
         * remains visible, and "a session that returns blank has lost the work
         * as far as its owner can tell". The transcript is in storage either
         * way; not reading it here was the whole of the difference.
         */
        let restored = self
            .stored_transcript(context_ref.as_deref())
            .await
            .unwrap_or_default();
        if !restored.is_empty() {
            model.apply(&restored);
            tracing::info!(session_id = %id, bytes = restored.len(), "transcript restored");
        }
        let screen: Arc<Mutex<Box<dyn ScreenModel>>> = Arc::new(Mutex::new(model));
        // What the capture sweep has already stored, so an unchanged screen is
        // not written back over itself on the next tick.
        let captured_len = screen.lock().await.repaint_bytes().len();

        let (terminal_input, terminal_output) = match terminal {
            Some(channel) => (Some(channel.input), Some(channel.output)),
            None => (None, None),
        };

        let mut entry = SessionEntry {
            session: seeded,
            sequences: Arc::clone(&self.sequences),
            subject_credential: spec.subject_credential.clone(),
            events: Vec::new(),
            screen: Arc::clone(&screen),
            runtime: Some(handle.clone()),
            input: terminal_input.clone(),
            live: live.clone(),
            live_frames: live_frames.clone(),
            conversation: conversation.as_ref().map(|c| c.input.clone()),
            pending_approvals: Default::default(),
            assembling: None,
            attached: None,
            attachments_issued: 0,
            context_ref,
            captured_len,
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

        // Both run for the session's life, independent of any client. A
        // session nobody is watching still paints its screen and still records
        // what its agent did, which is what makes the log a record rather than
        // a broadcast.
        if let Some(output) = terminal_output {
            spawn_relay(id.clone(), output, screen, live);
        }
        if let Some(channel) = conversation {
            // Recorded before anything is said on it. What this is for is a
            // control plane that has not started yet: the one replacing this
            // process reads the log to learn what the runtime calls the
            // conversation already running in it, and without that it opens a
            // second one beside it.
            if let Some(conversation_id) = named_conversation {
                let mut entries = self.entries.write().await;
                if let Some(entry) = entries.get_mut(&id) {
                    crate::service::append(
                        entry,
                        EventKind::ConversationOpened { conversation_id },
                    );
                }
            }
            crate::conversation_log::spawn_folder(Arc::clone(self), id.clone(), channel.updates);
        }

        // Handed over before the agent can need it, so the first turn is
        // already authenticated. Asked of the adapter rather than pushed down
        // the terminal channel: the credential's consumer is the supervisor's
        // proxy, and a runtime that renders no terminal still calls out on its
        // owner's behalf.
        if let Some(credential) = spec.subject_credential.as_deref() {
            if let Err(e) = adapter.refresh_credential(&handle, credential).await {
                // Logged rather than fatal, and never with the value. The
                // session is usable — someone can read it and type into it —
                // and what it cannot do is act on their behalf until a
                // credential lands. The top-up sweep is what retries that.
                tracing::warn!(
                    session_id = %id,
                    error = %e,
                    "the runtime started without its owner's credential"
                );
            }
        }
        if let Some(input) = &terminal_input {
            let _ = input
                .send(TerminalInput::Resize(TermSize {
                    cols: geometry.columns(),
                    rows: geometry.rows(),
                }))
                .await;
        }

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
        let seq = self.sequences.take(id, record.session.last_seq, 1);
        record.session.last_seq = seq;
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
}

impl SessionService {
    /// Give a session the granted set it should have, and record it.
    ///
    /// Two jobs that must not be separated. It refuses a session whose runtime
    /// can no longer be what the session is — the check every start makes —
    /// and it writes the answer down where the session was recorded before
    /// this model existed.
    ///
    /// The second job is why this exists. `granted` is the only thing that
    /// produces a set, and until now the two paths that bring a session *back*
    /// — readopting a surviving runtime, and starting a replacement — either
    /// copied the record verbatim or computed the value and discarded it. A
    /// session written by an older build therefore came back holding nothing,
    /// which is not "granted nothing": it is refused at every gate that asks,
    /// including the one behind every keystroke.
    ///
    /// Recorded rather than recomputed per process, because §5 makes the
    /// record the authority and a set that exists only in memory is one the
    /// next control plane has to derive again.
    pub(crate) async fn settle_capabilities(
        &self,
        session: &Session,
        adapter: &Arc<dyn KernelAdapter>,
    ) -> Result<SessionCapabilities, crate::error::CoreError> {
        let granted = SessionCapabilities::granted(session.options.mode, adapter.capabilities())?;
        if !session.capabilities.unrecorded() {
            return Ok(session.capabilities);
        }
        match self.store.get(&session.id).await {
            Ok(Some(mut record)) => {
                record.session.capabilities = granted;
                if let Err(e) = self.store.upsert(&record).await {
                    // Worth saying and not worth refusing over: the session
                    // works with the set in hand, and the next sweep settles
                    // it again.
                    tracing::warn!(session_id = %session.id, error = %e, "cannot record what this session holds");
                } else {
                    tracing::info!(
                        session_id = %session.id,
                        "settled what this session holds; it was recorded before capabilities existed"
                    );
                }
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(session_id = %session.id, error = %e, "cannot read this session to settle what it holds");
            }
        }
        Ok(granted)
    }
}

/// Translate what a person asked for into what a runtime is told.
///
/// The two vocabularies are separate on purpose — the core must not speak any
/// runtime's dialect — so this is the single place they meet, and the only
/// place a new option has to be taught to cross.
pub(crate) fn runtime_options(options: &AgentOptions) -> RuntimeOptions {
    RuntimeOptions {
        mode: match options.mode {
            crate::options::SessionMode::Terminal => layer3_adapter::RuntimeMode::Terminal,
            crate::options::SessionMode::Conversation => layer3_adapter::RuntimeMode::Conversation,
        },
        model: options.model.clone(),
        reasoning_effort: Some(options.reasoning_effort.as_str().to_string()),
        permission: match options.permission {
            PermissionMode::Guarded => RuntimePermission::Guarded,
            PermissionMode::Autonomous => RuntimePermission::Autonomous,
            PermissionMode::Unrestricted => RuntimePermission::Unrestricted,
        },
        own_credential: matches!(options.credential, crate::options::CredentialMode::Own),
        // Nothing to rejoin: this is a runtime being started, which has no
        // conversation yet. Taking one back over is the readopt path's, and it
        // reads the name from the record rather than from anything held here.
        resume_conversation: None,
    }
}
