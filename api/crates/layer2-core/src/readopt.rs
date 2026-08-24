//! Picking a session back up: reviving a stopped one, re-adopting a live one.
//!
//! Split from `provision` because the subject is different. That module brings
//! a runtime up for a session that has just been asked for; this one deals
//! with a session that already exists and whose runtime is either gone or
//! still there without anything attached to it.

use std::sync::Arc;

use layer3_adapter::RuntimeHandle;
use tokio::sync::{broadcast, watch, Mutex};

use crate::authz::{Caller, Capability};
use crate::error::CoreError;
use crate::event::{Event, EventKind};
use crate::ids::SessionId;
use crate::service::{store_failed, SessionEntry, SessionService, LIVE_BUFFER, RESOURCE_BUFFER};
use crate::session::{RecoveryState, Session, SessionSpec, SessionStatus};
use crate::store::SessionRecord;
use crate::terminal::ScreenModel;
use crate::terminal_io::spawn_relay;

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
        let lifecycle_gate = self.lifecycle_gates.gate(id);
        let _lifecycle = lifecycle_gate.lock().await;
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

        let settings = self.settings.for_provisioning().await;
        let limit_guard = self
            .refuse_at_session_limit(caller, settings.live_sessions)
            .await?;

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
        self.persist_durably(
            &mut starting,
            vec![Event {
                session_id: id.clone(),
                seq: 0,
                kind: EventKind::StatusChanged {
                    status: SessionStatus::Provisioning,
                    stop_reason: None,
                    detail: None,
                    capability: None,
                },
                appended_at: String::new(),
            }],
        )
        .await
        .map_err(store_failed)?;
        limit_guard.record_written(self.store.as_ref()).await;

        let geometry = starting.session.geometry;
        let spec = SessionSpec {
            geometry: Some(geometry),
            options: starting.session.options.clone(),
            environment_profile: starting.session.environment_profile.clone(),
            // The owner is not on the line to delegate one; the renewal loop
            // hands the session a fresh credential once it is running.
            caller_credential: None,
            runtime_credential: None,
            requested_mode: None,
            quoted_rate_card: None,
        };

        self.begin_start(id).await;
        let service = Arc::clone(self);
        let session = starting.session.clone();
        let spawned = session.clone();
        tokio::spawn(async move {
            service
                .bring_up(
                    spawned,
                    spec,
                    adapter,
                    geometry,
                    None,
                    crate::provision_failure::Abandonment::Keep,
                )
                .await;
        });

        tracing::info!(session_id = %id, "session reviving");
        Ok(session)
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
    /// What the runtime called this session's conversation, from the record.
    ///
    /// Read rather than remembered, which is the §5 rule applied to the one
    /// thing a replacement control plane cannot rebuild for itself. The last
    /// one wins: a session may have been rejoined before, and each rejoin
    /// records the name it ended up with.
    ///
    /// `None` where nothing named one — a session whose runtime has no name for
    /// a conversation, or one recorded before this was written down. The caller
    /// says so out loud rather than treating it as ordinary.
    async fn recorded_conversation(&self, id: &SessionId) -> Option<String> {
        let stored = self.store.events_since(id, 0).await.ok()?;
        stored
            .into_iter()
            .filter_map(crate::event::StoredEvent::into_known)
            .rev()
            .find_map(|event| match event.kind {
                crate::event::EventKind::ConversationOpened { conversation_id } => {
                    Some(conversation_id)
                }
                _ => None,
            })
    }

    pub(crate) async fn readopt(self: &Arc<Self>, record: &SessionRecord) -> Result<(), CoreError> {
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

        /*
         * Settled before anything is attached, for the same reason
         * provisioning settles it: a session recorded before this model
         * existed holds nothing, and a session that holds nothing is refused
         * at every gate that asks — including the one behind every keystroke.
         * Copying the record verbatim here is what left it that way.
         */
        let capabilities = self.settle_capabilities(&record.session, &adapter).await?;

        // Whichever plane this session was granted, and only that one — the
        // same rule provisioning follows, and for the same reason: the two are
        // exclusive, and a conversation session picked back up as a terminal
        // one is a session that is listed, intact, and unusable.
        let conversing = capabilities.conversation;
        /*
         * Whether the runtime is gone, or merely would not do this.
         *
         * These used to be the same answer, and the caller replaces a session's
         * runtime on it. So a bridge that answered — proving it was alive — was
         * reported as a sandbox that no longer exists, and the session was
         * rebuilt empty next to the pod still holding its conversation.
         *
         * The adapter already keeps the distinction: silence is `Unavailable`,
         * an answer is `Rejected`. This carries it rather than flattening it.
         */
        let unreachable = |e: layer3_adapter::AdapterError| {
            if e.is_runtime_loss() {
                tracing::warn!(session_id = %id, error = %e, "the runtime is gone");
                return CoreError::RuntimeUnavailable(
                    "the sandbox for this session is no longer running".to_string(),
                );
            }
            tracing::warn!(session_id = %id, error = %e, "the runtime refused the takeover");
            CoreError::RuntimeRefused(format!(
                "the sandbox for this session would not resume: {e}"
            ))
        };

        let terminal = match conversing {
            false => Some(
                adapter
                    .attach_terminal(&handle)
                    .await
                    .map_err(unreachable)?,
            ),
            true => None,
        };
        let conversation = match conversing {
            true => {
                // What the runtime calls the conversation already running in
                // it, read from the record because this process was not here
                // when it was opened. Rejoining is the whole point: without it
                // the runtime opens a second conversation beside the first and
                // the agent loses everything it knew.
                let mut options = crate::provision::runtime_options(&record.session.options);
                options.resume_conversation = self.recorded_conversation(&id).await;
                if options.resume_conversation.is_none() {
                    tracing::warn!(
                        session_id = %id,
                        "this session's conversation was never named; the runtime will be asked for a new one"
                    );
                }
                /*
                 * Folding starts before the open, for the reason given at the
                 * equivalent point in `provision.rs`: a conversation reports
                 * things while it is being opened, and a taken-back-over
                 * session is exactly where those reports matter — this is the
                 * path a control-plane replacement runs.
                 */
                let (updates_tx, updates_rx) =
                    tokio::sync::mpsc::channel(crate::provision::CONVERSATION_BUFFER);
                crate::conversation_log::spawn_folder(Arc::clone(self), id.clone(), updates_rx);
                Some(
                    adapter
                        .open_conversation(&handle, &options, updates_tx)
                        .await
                        .map_err(unreachable)?,
                )
            }
            false => None,
        };

        let (live, _) = broadcast::channel(LIVE_BUFFER);
        let (terminal_ending, _) = watch::channel(None);
        let (live_frames, _) = broadcast::channel(LIVE_BUFFER);
        let (resources, _) = broadcast::channel(RESOURCE_BUFFER);
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

        // Resolved again for redaction, not for provisioning: the runtime
        // survives and needs nothing, but a tool body it echoes must not carry
        // a value the session was handed. A profile that has since gone yields
        // nothing, which redacts nothing — honest incompleteness, not a guess.
        let environment_values: Vec<String> = self
            .environments
            .resolve_for_owner(
                &record.session.owner,
                record.session.environment_profile.as_ref(),
            )
            .await
            .map(|e| e.variables.values().cloned().collect())
            .unwrap_or_default();
        // A sweep or request can spend long enough attaching for the durable
        // desired state to change underneath it. Check again at the exact
        // installation boundary, under the same per-session gate used by stop
        // and termination, so neither can complete and then be locally undone
        // by this stale snapshot. The service-wide entries lock is never held
        // across the durable read or write.
        let installation_gate = self.lifecycle_gates.gate(&id);
        let installation = installation_gate.lock().await;
        if self.entries.read().await.contains_key(&id) {
            return Ok(());
        }
        let mut current = self
            .store
            .get(&id)
            .await
            .map_err(store_failed)?
            .ok_or(CoreError::SessionNotFound)?;
        if current.session.status.is_terminal() {
            return Err(CoreError::SessionTerminated);
        }
        if !current.session.status.wants_runtime()
            || current.session.kernel != record.session.kernel
            || current.runtime_id.as_deref() != Some(handle.0.as_str())
        {
            return Err(CoreError::RuntimeUnavailable(
                "the session changed while its runtime was being reconnected".into(),
            ));
        }

        // A reachable runtime proves that a recorded provisioning state has
        // completed. Make that fact durable before publishing the entry; doing
        // it afterwards leaves a window where a concurrent stop can commit
        // idle and then be overwritten by this stale ready write.
        if current.session.status == SessionStatus::Provisioning {
            current.session.status = SessionStatus::Ready;
            current.session.recovery_state = RecoveryState::Normal;
            current.session.stop_reason = None;
            self.persist_durably(
                &mut current,
                vec![Event {
                    session_id: id.clone(),
                    seq: 0,
                    kind: EventKind::StatusChanged {
                        status: SessionStatus::Ready,
                        stop_reason: None,
                        detail: None,
                        capability: None,
                    },
                    appended_at: String::new(),
                }],
            )
            .await
            .map_err(store_failed)?;
        }

        let owner = current.session.owner.clone();
        let mut adopted = current.session.clone();
        adopted.capabilities = capabilities;
        let event_log = self.event_logs.lane(&id);
        event_log.observe_tail(adopted.last_seq);
        event_log.observe_record(&current);
        let entry = SessionEntry {
            session: adopted,
            event_log,
            subject_credential: None,
            environment_values,
            screen: Arc::clone(&screen),
            runtime: Some(handle.clone()),
            input: terminal.as_ref().map(|c| c.input.clone()),
            live: Some(live.clone()),
            terminal_ending: terminal_ending.clone(),
            live_frames: live_frames.clone(),
            resources,
            last_reading: None,
            memory_pressure: Default::default(),
            readings_unreported: false,
            conversation: conversation.as_ref().map(|c| c.input.clone()),
            pending_approvals: Default::default(),
            assembling: None,
            context_reading: None,
            attached: None,
            attachments_issued: 0,
            context_ref: current.context_ref.clone(),
            captured_len,
        };
        self.entries.write().await.insert(id.clone(), entry);
        drop(installation);
        crate::resources::spawn_resource_watch(
            Arc::clone(self),
            id.clone(),
            Arc::clone(&adapter),
            handle,
        );
        if let Some(channel) = terminal {
            spawn_relay(
                Arc::clone(self),
                id.clone(),
                channel.output,
                screen,
                live,
                terminal_ending,
            );
        }

        // The sandbox kept running, but the credential it was given belonged to
        // a process that has gone. Without this the session answers every
        // prompt with an authorisation failure.
        self.supply_credential(&id, &owner).await;

        tracing::info!(session_id = %id, "reconnected to an existing runtime");
        Ok(())
    }
}
