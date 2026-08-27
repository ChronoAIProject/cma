//! Picking a session back up: reviving a stopped one, re-adopting a live one.
//!
//! Split from `provision` because the subject is different. That module brings
//! a runtime up for a session that has just been asked for; this one deals
//! with a session that already exists and whose runtime is either gone or
//! still there without anything attached to it.

use std::{collections::HashMap, sync::Arc};

use layer3_adapter::{ConversationUpdate, PermissionLocator, RuntimeHandle};
use tokio::sync::{broadcast, watch, Mutex, OwnedMutexGuard};

use crate::authz::{Caller, Capability};
use crate::error::CoreError;
use crate::event::{Event, EventKind};
use crate::ids::SessionId;
use crate::service::{store_failed, SessionEntry, SessionService, LIVE_BUFFER, RESOURCE_BUFFER};
use crate::session::{RecoveryState, Session, SessionSpec, SessionStatus};
use crate::store::{SessionRecord, StoreError};
use crate::terminal::ScreenModel;
use crate::terminal_io::spawn_relay;
use crate::SessionMode;

/// Durable conversation state a replacement process needs before it attaches.
#[derive(Default)]
pub(crate) struct ReadoptionHistory {
    pub(crate) conversation_id: Option<String>,
    pub(crate) pending_approvals: HashMap<String, Option<PermissionLocator>>,
    pub(crate) approval_locators: HashMap<String, Option<PermissionLocator>>,
}

/// One serialized attempt to rebuild a process-local runtime attachment.
///
/// Holding the permit inside this value makes it impossible to read the
/// durable snapshot and then accidentally release exclusivity before the
/// socket and its folder have been installed.
pub(crate) struct Readoption {
    _permit: OwnedMutexGuard<()>,
    history: ReadoptionHistory,
}

impl SessionService {
    /// Settle a legacy handover against the exact runtime being readopted.
    ///
    /// H1 keeps its old public grant. H2 publishes the candidate grant and
    /// custody in the same Ready commit. The source identity is deliberately a
    /// handle/generation pair: generation zero compatibility handovers use the
    /// descriptor to distinguish their handle-only authority, while coordinated
    /// switches must advance the generation monotonically.
    fn settle_readopted_legacy_handover(
        record: &mut SessionRecord,
        runtime_generation: u64,
    ) -> Result<(), StoreError> {
        let crate::store::SkillPackState::LegacyHandover {
            source_runtime_id,
            source_generation,
            packs,
            stored_digest,
        } = record.skill_packs.clone()
        else {
            return Ok(());
        };
        if source_runtime_id.is_empty() {
            return Err(StoreError::Malformed(
                "a legacy skill handover has no exact source runtime".into(),
            ));
        }
        crate::store::validate_skill_packs(&packs).map_err(|error| {
            StoreError::Malformed(format!("invalid legacy handover skill packs: {error}"))
        })?;
        if stored_digest
            .is_some_and(|digest| crate::store::SkillPackDigest::for_packs(&packs) != digest)
        {
            return Err(StoreError::Malformed(
                "a legacy skill handover receipt does not match its exact bytes".into(),
            ));
        }
        let runtime_id = record
            .runtime_id
            .as_deref()
            .filter(|runtime_id| !runtime_id.is_empty())
            .ok_or_else(|| {
                StoreError::Malformed(
                    "a readopted legacy skill handover has no authoritative runtime".into(),
                )
            })?;
        if let Some(pending) = record.pending_handover.as_ref() {
            if record.rotation_claim.as_deref() != Some(pending.claim.as_str()) {
                return Err(StoreError::Malformed(
                    "a compatibility skill handover does not own its rotation claim".into(),
                ));
            }
        }

        let source_survived = if source_generation == 0 {
            if record.pending_handover.is_some() {
                if runtime_generation != 0 {
                    return Err(StoreError::Malformed(
                        "a compatibility skill handover acquired nonzero runtime authority".into(),
                    ));
                }
                runtime_id == source_runtime_id
            } else {
                match runtime_generation {
                    0 if runtime_id == source_runtime_id => true,
                    0 => {
                        return Err(StoreError::Malformed(
                            "a generation-zero skill handover changed runtime without a compatibility descriptor"
                                .into(),
                        ))
                    }
                    _ if runtime_id != source_runtime_id => false,
                    _ => {
                        return Err(StoreError::Malformed(
                            "a legacy skill handover advanced authority without changing runtimes"
                                .into(),
                        ))
                    }
                }
            }
        } else if runtime_generation < source_generation {
            return Err(StoreError::Malformed(
                "a legacy skill handover runtime generation regressed".into(),
            ));
        } else if runtime_generation == source_generation {
            if runtime_id != source_runtime_id {
                return Err(StoreError::Malformed(
                    "a legacy skill handover changed handles without advancing authority".into(),
                ));
            }
            true
        } else if runtime_id == source_runtime_id {
            return Err(StoreError::Malformed(
                "a legacy skill handover advanced authority without changing runtimes".into(),
            ));
        } else {
            false
        };

        if source_survived {
            // A successful object-store receipt lets H1 return to its exact
            // legacy state while the candidate remains at the deterministic key
            // for the next retry. Without external custody, retain the inline
            // candidate so a retry still never consults the mutable registry.
            if stored_digest.is_some() {
                record.skill_packs = crate::store::SkillPackState::Legacy;
            }
            return Ok(());
        }
        if record.rotation_claim.is_none() && record.session.status != SessionStatus::Provisioning {
            return Err(StoreError::Malformed(
                "a replacement runtime cannot settle legacy skills without its rotation claim"
                    .into(),
            ));
        }

        record.session.granted_skills = Some(Self::granted_skills_for(&packs));
        record.skill_packs = stored_digest.map_or_else(
            || crate::store::SkillPackState::Staged(packs),
            |digest| crate::store::SkillPackState::Stored { digest },
        );
        Ok(())
    }

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
        let _lifecycle = self.lock_lifecycle(id).await;
        let record = self.record(id).await?;
        if !record.session.is_owned_by(&caller.subject) {
            return Err(CoreError::Forbidden);
        }
        if record.session.status.is_terminal() {
            return Err(CoreError::SessionTerminated);
        }
        if record.session.wants_runtime() {
            // Already running or already starting. Reviving again would
            // provision a second sandbox for one session.
            return Ok(record.session);
        }
        if record.runtime_id.is_some() {
            return Err(CoreError::RuntimeUnavailable(
                "the stopped session's previous runtime cleanup is still pending".into(),
            ));
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

        if !self.begin_start(id).await {
            return Err(CoreError::InvalidState);
        }

        let (record, restore, trigger_recovery) =
            if let Some(pending) = record.pending_handover.as_ref() {
                let recovery = match self.load_pending_handover_recovery(&record).await {
                    Ok(recovery) => recovery,
                    Err(error) => {
                        self.finish_start(id).await;
                        return Err(store_failed(error));
                    }
                };
                let mut rotating = record.clone();
                rotating.session.status = SessionStatus::Rotating;
                rotating.session.stop_reason = None;
                rotating.rotation_claim = Some(pending.claim.clone());
                if let Err(error) = self
                    .persist_durably(
                        &mut rotating,
                        vec![Event {
                            session_id: id.clone(),
                            seq: 0,
                            kind: EventKind::StatusChanged {
                                status: SessionStatus::Rotating,
                                stop_reason: None,
                                detail: Some(
                                    "resuming the stored runtime handover from its exact snapshot"
                                        .into(),
                                ),
                                capability: None,
                            },
                            appended_at: String::new(),
                        }],
                    )
                    .await
                {
                    self.finish_start(id).await;
                    return Err(store_failed(error));
                }
                let published = match self.publish_pending_handover_target(rotating).await {
                    Ok(published) => published,
                    Err(error) => {
                        self.finish_start(id).await;
                        return Err(store_failed(error));
                    }
                };
                (published, recovery.restore, recovery.trigger_recovery)
            } else {
                let trigger_recovery =
                    match self.resolve_trigger_recovery_pair_for(&record, None).await {
                        Ok(pair) => pair,
                        Err(error) => {
                            self.finish_start(id).await;
                            return Err(store_failed(error));
                        }
                    };
                if record.trigger_origin.is_some() && trigger_recovery.is_none() {
                    self.finish_start(id).await;
                    return Err(CoreError::RuntimeUnavailable(
                        "this Trigger session has no complete workspace/conversation recovery pair"
                            .into(),
                    ));
                }
                (record, None, trigger_recovery)
            };

        let mut lease = match self
            .acquire_runtime_lease_at(&record, crate::coordination::SessionOperation::Provision)
            .await
        {
            Ok(lease) => lease,
            Err(error) => {
                self.finish_start(id).await;
                return Err(store_failed(error));
            }
        };

        let mut starting = record.clone();
        starting.session.status = SessionStatus::Provisioning;
        starting.session.stop_reason = None;
        if starting.pending_handover.is_none() {
            // Older failure paths could leave a private claim without a
            // recoverable descriptor. Ordinary revive repairs that stale
            // fence; a descriptor-backed revive preserves its exact claim.
            starting.rotation_claim = None;
        }
        if let Err(error) = self
            .persist_durably(
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
        {
            if let Some(held) = lease.as_ref() {
                let _ = self.store.release_session_lease(held).await;
            }
            self.finish_start(id).await;
            return Err(store_failed(error));
        }
        if let Some(held) = lease.as_mut() {
            let boundary = crate::coordination::SessionBoundary::for_record(
                &starting,
                held.boundary.authority.generation,
            );
            if let Err(error) = self.advance_runtime_lease(held, &boundary).await {
                let _ = self.store.release_session_lease(held).await;
                self.finish_start(id).await;
                return Err(store_failed(error));
            }
        }
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

        let service = Arc::clone(self);
        let session = starting.session.clone();
        let rotation_claim = starting.rotation_claim.clone();
        let spawned = session.clone();
        tokio::spawn(async move {
            service
                .bring_up_with_lease(
                    spawned,
                    adapter,
                    spec,
                    geometry,
                    restore,
                    crate::provision_support::BringUpContext {
                        outcome: crate::provision_failure::Abandonment::Keep,
                        lease,
                        handover: None,
                        trigger_recovery,
                        prepared_skills: None,
                        rotation_claim,
                    },
                )
                .await;
        });

        tracing::info!(session_id = %id, "session reviving");
        Ok(session)
    }

    /// Read the durable facts needed to rebuild one conversation attachment.
    ///
    /// Kept separate from the network work so reconciliation can defer safely
    /// when history is unavailable. Attaching first and discovering that the
    /// answerable set cannot be rebuilt would leave a live runtime claimed by
    /// neither the old process nor the replacement.
    pub(crate) async fn readoption_history(
        &self,
        record: &SessionRecord,
    ) -> Result<ReadoptionHistory, StoreError> {
        if record.session.options.mode != SessionMode::Conversation {
            return Ok(ReadoptionHistory::default());
        }
        let id = &record.session.id;
        let stored_events = self.store.events_since(id, 0).await?;
        let mut durable_events = Vec::with_capacity(stored_events.len());
        for stored in stored_events {
            let seq = stored.seq;
            match stored.kind {
                crate::event::StoredEventKind::Known(kind) => durable_events.push(Event {
                    session_id: stored.session_id,
                    seq,
                    kind,
                    appended_at: stored.appended_at,
                }),
                crate::event::StoredEventKind::Unknown { tag, .. } => {
                    return Err(StoreError::Malformed(format!(
                        "cannot restore approval state past unknown event {tag} at sequence {seq}"
                    )))
                }
            }
        }
        let conversation_id = durable_events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                EventKind::ConversationOpened { conversation_id } => Some(conversation_id.clone()),
                _ => None,
            });
        let approvals = crate::conversation_log::pending_approvals_from(&durable_events)
            .map_err(StoreError::Malformed)?;
        Ok(ReadoptionHistory {
            conversation_id,
            pending_approvals: approvals.pending,
            approval_locators: approvals.locators,
        })
    }

    /// Serialize and prepare one readoption before touching the runtime.
    ///
    /// The entry check is deliberately after acquiring the permit. Callers
    /// commonly arrive together after a process restart; a check before the
    /// gate lets each of them open a socket and spawn a folder before only one
    /// can win installation.
    pub(crate) async fn prepare_readoption(
        &self,
        record: &SessionRecord,
    ) -> Result<Option<Readoption>, StoreError> {
        let id = &record.session.id;
        let permit = self.readoption_gates.gate(id).lock_owned().await;
        if self.entries.read().await.contains_key(id) {
            return Ok(None);
        }
        let history = self.readoption_history(record).await?;
        Ok(Some(Readoption {
            _permit: permit,
            history,
        }))
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
    pub(crate) async fn readopt(self: &Arc<Self>, record: &SessionRecord) -> Result<(), CoreError> {
        let service = Arc::clone(self);
        let record = record.clone();
        let task_id = record.session.id.clone();
        let readoption = tokio::spawn(async move {
            // The task owns the complete operation once it starts. A caller
            // whose wait is cancelled detaches instead of abandoning durable
            // preparation or an adapter channel halfway through opening it.
            let Some(readoption) = service
                .prepare_readoption(&record)
                .await
                .map_err(store_failed)?
            else {
                return Ok(());
            };
            service.readopt_prepared(&record, readoption).await
        });

        readoption.await.map_err(|error| {
            tracing::error!(session_id = %task_id, %error, "session readoption task failed");
            CoreError::RuntimeUnavailable(
                "session readoption stopped before it could complete".into(),
            )
        })?
    }

    pub(crate) async fn readopt_prepared(
        self: &Arc<Self>,
        record: &SessionRecord,
        readoption: Readoption,
    ) -> Result<(), CoreError> {
        let Readoption {
            _permit: _readoption,
            history,
        } = readoption;
        let mut lease = match self
            .acquire_runtime_lease_at(record, crate::coordination::SessionOperation::Readopt)
            .await
        {
            Ok(lease) => lease,
            Err(error @ StoreError::SessionAuthorityChanged { .. }) => {
                let current = self
                    .store
                    .get(&record.session.id)
                    .await
                    .map_err(store_failed)?;
                if current.is_some_and(|record| record.session.status.is_terminal()) {
                    return Err(CoreError::SessionTerminated);
                }
                return Err(store_failed(error));
            }
            Err(error) => return Err(store_failed(error)),
        };
        let publication_token = format!("publication_{}", uuid::Uuid::new_v4().simple());
        let result = self
            .readopt_with_lease(record, history, &mut lease, &publication_token)
            .await;
        if result.is_err() {
            self.remove_incomplete_runtime_entry(&record.session.id, &publication_token)
                .await;
        }
        if let Some(held) = lease.take() {
            if let Err(error) = self.store.release_session_lease(&held).await {
                tracing::debug!(
                    session_id = %record.session.id,
                    %error,
                    "readoption lease ended before explicit release"
                );
            }
        }
        result
    }

    async fn readopt_with_lease(
        self: &Arc<Self>,
        record: &SessionRecord,
        history: ReadoptionHistory,
        lease: &mut Option<crate::coordination::SessionLease>,
        publication_token: &str,
    ) -> Result<(), CoreError> {
        let id = record.session.id.clone();
        let ReadoptionHistory {
            conversation_id,
            pending_approvals: _,
            approval_locators: _,
        } = history;
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

        self.renew_runtime_lease(lease.as_ref())
            .await
            .map_err(store_failed)?;
        let terminal = match conversing {
            false => Some(
                adapter
                    .attach_terminal(&handle)
                    .await
                    .map_err(unreachable)?,
            ),
            true => None,
        };
        let (conversation, mut conversation_opening) = match conversing {
            true => {
                self.renew_runtime_lease(lease.as_ref())
                    .await
                    .map_err(store_failed)?;
                // What the runtime calls the conversation already running in
                // it, read from the same durable snapshot as pending approvals.
                // The last recorded name wins because a session may have been
                // rejoined before. Rejoining is the whole point: without it the
                // runtime opens a second conversation beside the first.
                let mut options = crate::provision::runtime_options(&record.session.options);
                options.resume_conversation = conversation_id.clone();
                options.restricted_service_gateway = record.trigger_origin.is_some();
                if options.resume_conversation.is_none() {
                    tracing::warn!(
                        session_id = %id,
                        "this session's conversation was never named; the runtime will be asked for a new one"
                    );
                }
                let opening_authority = self
                    .acquire_conversation_opening(
                        &id,
                        &handle.0,
                        lease
                            .as_ref()
                            .map_or(0, |held| held.boundary.authority.generation),
                        None,
                        lease.as_ref(),
                    )
                    .await
                    .map_err(store_failed)?;
                let (updates_tx, mut opening) =
                    crate::conversation_opening::ConversationOpening::channel(
                        crate::provision::CONVERSATION_BUFFER,
                        opening_authority,
                        Arc::clone(&self.store),
                    );
                let renewal_authority = opening.authority().clone();
                let opening_result: Result<_, CoreError> = {
                    let open = adapter.open_conversation(&handle, &options, updates_tx);
                    tokio::pin!(open);
                    let renewal = tokio::time::sleep(
                        crate::provision_support::CONVERSATION_OPENING_RENEW_INTERVAL,
                    );
                    tokio::pin!(renewal);
                    let mut receiving = true;
                    loop {
                        tokio::select! {
                            result = &mut open => break Ok(result),
                            () = &mut renewal => {
                                if let Err(error) = self
                                    .renew_conversation_opening(
                                        &renewal_authority,
                                        lease.as_ref(),
                                    )
                                    .await
                                {
                                    break Err(store_failed(error));
                                }
                                renewal.as_mut().reset(
                                    tokio::time::Instant::now()
                                        + crate::provision_support::CONVERSATION_OPENING_RENEW_INTERVAL,
                                );
                            }
                            update = opening.receive(), if receiving => match update {
                                Some(ConversationUpdate::SignInRequired { url, code, message }) => {
                                    if !opening.can_publish_sign_in() {
                                        break Err(CoreError::RuntimeRefused(
                                            "the runtime emitted an ordinary conversation update before its opening sign-in prompt".into(),
                                        ));
                                    }
                                    if let Err(error) = self
                                        .persist_opening_sign_in(
                                            &id,
                                            &renewal_authority,
                                            lease.as_ref(),
                                            url,
                                            code,
                                            message,
                                        )
                                        .await
                                    {
                                        break Err(store_failed(error));
                                    }
                                }
                                Some(update) => opening.hold(update),
                                None => receiving = false,
                            }
                        }
                    }
                };
                match opening_result {
                    Ok(Ok(channel)) => (Some(channel), Some(opening)),
                    Ok(Err(error)) => {
                        self.revoke_conversation_opening(&id, opening).await;
                        return Err(unreachable(error));
                    }
                    Err(error) => {
                        self.revoke_conversation_opening(&id, opening).await;
                        return Err(error);
                    }
                }
            }
            false => (None, None),
        };

        let mut conversation_publication = None;
        let result = async {

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
        let installation = self.lock_lifecycle(&id).await;
        let input_fence = self.input_gates.gate(&id).write_owned().await;
        let ordering = self.conversation_order_gates.gate(&id);
        let ordered_installation = ordering.lock().await;
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
        let status_changed_by_conversation = matches!(
            current.session.status,
            SessionStatus::Ready | SessionStatus::Running
        ) || (current.session.status == SessionStatus::Idle
            && current.session.stop_reason == Some(crate::session::StopReason::RequiresAction));
        if !current.session.wants_runtime()
            || (current.session.status != record.session.status && !status_changed_by_conversation)
            || current.session.kernel != record.session.kernel
            || current.runtime_id.as_deref() != Some(handle.0.as_str())
        {
            return Err(CoreError::RuntimeUnavailable(
                "the session changed while its runtime was being reconnected".into(),
            ));
        }
        if let Some(held) = lease.as_mut() {
            let current_boundary = crate::coordination::SessionBoundary::for_record(
                &current,
                held.boundary.authority.generation,
            );
            if current_boundary == held.boundary {
                self.renew_runtime_lease(Some(held))
                    .await
                    .map_err(store_failed)?;
            } else {
                // A rejoined runtime can report a request, completion, or
                // resolution while its channel is opening. Those conversation
                // updates change status without changing runtime authority. The
                // store advances only if this lease is still current and the
                // record is exactly the boundary just validated above.
                self.advance_runtime_lease(held, &current_boundary)
                    .await
                    .map_err(store_failed)?;
            }
        }

        // A reachable runtime proves that a recorded provisioning state has
        // completed. It can also safely cancel an interrupted handover whose
        // still-authoritative runtime survived the process that claimed it;
        // the next reconciliation may start a fresh version rotation. Mint the
        // legacy claim before leaving `Rotating` so every other replica stays
        // fenced until this exact readoption is locally published.
        //
        // Make that fact durable before publishing the entry; doing it
        // afterwards leaves a window where a concurrent stop can commit idle
        // and then be overwritten by this stale ready write.
        if matches!(
            current.session.status,
            SessionStatus::Provisioning | SessionStatus::Rotating
        ) {
            let ready_claimed = current.clone();
            if current.session.status.is_rotating() && current.rotation_claim.is_none() {
                if matches!(
                    &current.skill_packs,
                    crate::store::SkillPackState::LegacyHandover { .. }
                ) {
                    return Err(store_failed(StoreError::Malformed(
                        "a rotating legacy skill handover has no durable claim".into(),
                    )));
                }
                current.rotation_claim = Some(format!(
                    "rotn_{}",
                    uuid::Uuid::new_v4().simple()
                ));
            }
            let runtime_generation = lease
                .as_ref()
                .map_or(0, |held| held.boundary.authority.generation);
            Self::settle_readopted_legacy_handover(&mut current, runtime_generation)
                .map_err(store_failed)?;
            current.session.status = SessionStatus::Ready;
            current.session.recovery_state = RecoveryState::Normal;
            current.session.stop_reason = None;
            self.publish_ready_durably(
                &mut current,
                &ready_claimed,
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
            if let Some(held) = lease.as_mut() {
                let boundary = crate::coordination::SessionBoundary::for_record(
                    &current,
                    held.boundary.authority.generation,
                );
                self.advance_runtime_lease(held, &boundary)
                    .await
                    .map_err(store_failed)?;
            }
        }

        // Rebuild from the final durable history after any readiness event.
        // The opening FIFO is still private and will be folded immediately
        // after this state is installed under the same two input/order gates.
        let final_history = self
            .readoption_history(&current)
            .await
            .map_err(|error| {
                CoreError::RuntimeRefused(format!(
                    "the runtime was reached but its final conversation state could not be rebuilt: {error}"
                ))
            })?;
        if final_history.conversation_id != conversation_id {
            return Err(CoreError::RuntimeRefused(
                "the conversation changed while its runtime was being reconnected".into(),
            ));
        }
        let pending_approvals = final_history.pending_approvals;
        let approval_locators = final_history.approval_locators;

        if !pending_approvals.is_empty() {
            tracing::info!(
                session_id = %id,
                count = pending_approvals.len(),
                "pending approvals restored during readoption"
            );
        }

        let (conversation_link, conversation_authority, opening_updates, conversation_updates) =
            match conversation_opening.take() {
                Some(opening) => {
                    if let Err(error) = self
                        .renew_conversation_opening(opening.authority(), lease.as_ref())
                        .await
                    {
                        opening.revoke().await;
                        return Err(store_failed(error));
                    }
                    let (link, durable, held, updates, publication) = opening
                        .publish(lease.as_ref())
                        .await
                        .map_err(store_failed)?;
                    conversation_publication = Some(publication);
                    (Some(link), Some(durable), Some(held), Some(updates))
                }
                None => (None, None, None, None),
            };

        let runtime_generation = lease
            .as_ref()
            .map_or_else(
                || {
                    current
                        .trigger_origin
                        .as_ref()
                        .map(|_| u64::from(current.session.rotations).saturating_add(1))
                        .unwrap_or(0)
                },
                |held| held.boundary.authority.generation,
            );
        if let (Some(workspaces), Some(origin)) =
            (self.workspaces.as_ref(), current.trigger_origin.as_ref())
        {
            workspaces
                .synchronize_runtime_generation(
                    &current.session.owner,
                    origin,
                    runtime_generation,
                    &crate::clock::now_rfc3339(),
                )
                .await
                .map_err(store_failed)?;
        }

        let mut adopted = current.session.clone();
        adopted.capabilities = capabilities;
        let event_log = self.event_logs.lane(&id);
        event_log.observe_tail(adopted.last_seq);
        event_log.observe_record(&current);
        let entry = SessionEntry {
            session: adopted,
            trigger_origin: current.trigger_origin.clone(),
            initial_prompt: current.initial_prompt.clone(),
            event_log,
            subject_credential: None,
            environment_values,
            screen: Arc::clone(&screen),
            runtime: Some(handle.clone()),
            runtime_generation,
            publication_token: publication_token.to_string(),
            publication_complete: false,
            conversation_link,
            conversation_authority,
            input: terminal.as_ref().map(|c| c.input.clone()),
            live: Some(live.clone()),
            terminal_ending: terminal_ending.clone(),
            live_frames: live_frames.clone(),
            resources,
            last_reading: None,
            memory_pressure: Default::default(),
            readings_unreported: false,
            conversation: conversation.as_ref().map(|c| c.input.clone()),
            pending_approvals,
            approval_locators,
            assembling: None,
            context_reading: None,
            attached: None,
            attachments_issued: 0,
            context_ref: current.context_ref.clone(),
            skill_packs: current.skill_packs.clone(),
            captured_len,
        };
        self.entries.write().await.insert(id.clone(), entry);
        if let (Some(link), Some(updates)) = (conversation_link, opening_updates) {
            if !self.fold_opening_updates_locked(&id, link, updates).await {
                return Err(CoreError::RuntimeRefused(
                    "the conversation link was superseded during opening replay".into(),
                ));
            }
        }
        if let Some(held) = lease.as_mut() {
            let replayed = self
                .store
                .get(&id)
                .await
                .map_err(store_failed)?
                .ok_or(CoreError::SessionNotFound)?;
            let boundary =
                crate::coordination::SessionBoundary::for_record(&replayed, runtime_generation);
            if boundary != held.boundary {
                self.advance_runtime_lease(held, &boundary)
                    .await
                    .map_err(store_failed)?;
            }
        }
        // Readoption already owns the exact durable runtime boundary. Use it
        // to install and acknowledge custody before this entry becomes valid
        // evidence that a retained rotation claim may clear. Waiting until
        // lease release would let an ordinary prompt win the credential race.
        if self.credentials.is_some() {
            match self
                .install_prepared_credential_while_lifecycle_locked(
                    &installation,
                    &current.session,
                    None,
                    &handle,
                    runtime_generation,
                    lease.as_ref(),
                )
                .await
            {
                Ok(true) => {}
                Ok(false) => tracing::warn!(
                    session_id = %id,
                    "the readopted runtime resumed without its owner's credential"
                ),
                Err(error) => tracing::warn!(
                    session_id = %id,
                    %error,
                    "the readopted runtime's credential could not be recorded"
                ),
            }
        }
        {
            let mut entries = self.entries.write().await;
            let exact = entries.get(&id).is_some_and(|entry| {
                entry.runtime.as_ref() == Some(&handle)
                    && entry.runtime_generation == runtime_generation
                    && entry.publication_token == publication_token
                    && Arc::ptr_eq(&entry.screen, &screen)
            });
            if !exact {
                return Err(CoreError::RuntimeUnavailable(
                    "the runtime entry changed before readoption completed".into(),
                ));
            }
            if let (Some(link), Some(updates)) = (conversation_link, conversation_updates) {
                crate::conversation_log::spawn_folder(
                    Arc::clone(self),
                    id.clone(),
                    link,
                    runtime_generation,
                    updates,
                );
                conversation_publication
                    .take()
                    .expect("a published conversation receiver retains its cleanup guard")
                    .disarm();
            }
            crate::resources::spawn_resource_watch(
                Arc::clone(self),
                id.clone(),
                Arc::clone(&adapter),
                handle.clone(),
            );
            if let Some(channel) = terminal {
                spawn_relay(
                    Arc::clone(self),
                    id.clone(),
                    channel.output,
                    Arc::clone(&screen),
                    live,
                    terminal_ending,
                );
            }
            entries
                .get_mut(&id)
                .expect("the exact readopted entry remains locked")
                .publication_complete = true;
        }
        drop(ordered_installation);
        drop(input_fence);
        drop(installation);
        if let Some(claim) = current.rotation_claim.as_deref() {
            match self.finalize_rotation_publication(&id, claim).await {
                Ok(true) => {}
                Ok(false) => tracing::warn!(
                    session_id = %id,
                    "the readopted runtime no longer owns its rotation publication claim"
                ),
                Err(error) => tracing::warn!(
                    session_id = %id,
                    %error,
                    "the readopted runtime remains interaction-fenced until reconciliation clears its rotation claim"
                ),
            }
        }
        if let Some(held) = lease.take() {
            if let Err(error) = self.store.release_session_lease(&held).await {
                tracing::debug!(
                    session_id = %id,
                    %error,
                    "readoption lease ended before explicit release"
                );
            }
        }

        tracing::info!(session_id = %id, "reconnected to an existing runtime");
        Ok(())
        }
        .await;
        if let Some(publication) = conversation_publication.take() {
            self.revoke_conversation_publication(&id, publication).await;
        }
        if let Some(opening) = conversation_opening.take() {
            self.revoke_conversation_opening(&id, opening).await;
        }
        result
    }
}
