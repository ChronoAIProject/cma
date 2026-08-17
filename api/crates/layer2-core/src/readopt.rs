//! Picking a session back up: reviving a stopped one, re-adopting a live one.
//!
//! Split from `provision` because the subject is different. That module brings
//! a runtime up for a session that has just been asked for; this one deals
//! with a session that already exists and whose runtime is either gone or
//! still there without anything attached to it.

use std::sync::Arc;

use layer3_adapter::RuntimeHandle;
use tokio::sync::{broadcast, Mutex};

use crate::authz::{Caller, Capability};
use crate::error::CoreError;
use crate::event::{Event, EventKind};
use crate::ids::SessionId;
use crate::service::{spawn_relay, store_failed, SessionEntry, SessionService, LIVE_BUFFER};
use crate::session::{Session, SessionSpec, SessionStatus};
use crate::store::SessionRecord;
use crate::terminal::ScreenModel;

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
            environment_profile: starting.session.environment_profile.clone(),
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
        let (live_frames, _) = broadcast::channel(LIVE_BUFFER);
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
            live_frames: live_frames.clone(),
            conversation: None,
            pending_approvals: Default::default(),
            assembling: None,
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
