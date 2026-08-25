//! Carrying a session onto another runtime without interrupting it.
//!
//! Two things ask for this and both are the same act. The runtime image moves
//! on and sessions started on the old one have to follow it; or an owner asks
//! to be shown their session as a conversation rather than as a screen, which
//! means the same agent under a different front-end. Either way the session is
//! captured, its sandbox is released, a replacement is started, and the agent
//! resumes from the capture — so this is written once and told what changes.
//!
//! Upgrading a sandbox in place is not an option: the image is the sandbox.
//! So the session is moved instead, and its identifier, its history, its
//! configuration and its place in its owner's list are all unchanged, because
//! none of those were ever properties of the sandbox.
//!
//! Two things make this honest rather than merely invisible.
//!
//! The snapshot has a moment. Anything the agent did after it is not carried
//! across, so the moment is recorded and shown, and the owner can tell what
//! was included rather than guessing why something is missing.
//!
//! The session refuses input while it happens. Accepting keystrokes bound for
//! a sandbox that is being torn down would lose them silently, which is worse
//! than briefly declining them and saying so.
//!
//! The snapshot itself is opaque here. It is captured by an adapter, stored by
//! the core and handed back to an adapter, and nothing along that path reads
//! it — only the agent that produced one knows how. That is what lets a new
//! kernel be added without teaching this file anything about it.

use std::sync::Arc;

use layer3_adapter::RuntimeHandle;

use crate::authz::Caller;
use crate::capabilities::SessionCapabilities;
use crate::clock::now_rfc3339;
use crate::error::CoreError;
use crate::event::{Event, EventKind};
use crate::ids::SessionId;
use crate::options::SessionMode;
use crate::service::SessionService;
use crate::session::{Session, SessionSpec, SessionStatus};
use crate::store::SessionRecord;

/// What a handover changes about the session it carries.
///
/// The only difference between the two, and it is deliberately small: a
/// handover is a handover, and forking the path would give the rarer one a
/// second set of failure cases nobody exercises.
enum Handover {
    /// Onto the runtime version this deployment expects.
    Version(String),
    /// Onto the other plane, because the session's owner asked.
    Plane {
        mode: SessionMode,
        /// What the session holds afterwards, derived before anything is torn
        /// down so the move cannot get halfway and find out it is impossible.
        capabilities: SessionCapabilities,
    },
}

impl Handover {
    /// What the owner is told while it happens.
    fn detail(&self) -> &'static str {
        match self {
            Self::Version(_) => {
                "moving this session to an updated runtime; it will come back where it left off"
            }
            Self::Plane { .. } => {
                "moving this session to the other plane; it will come back where it left off"
            }
        }
    }
}

impl SessionService {
    /// Whether this session is running something other than what it should be.
    ///
    /// A session with no recorded version counts as behind: it predates
    /// version tracking, so nothing can vouch for what it is running, and
    /// leaving it there forever is the one outcome that is certainly wrong.
    pub(crate) fn needs_rotation(&self, record: &SessionRecord) -> bool {
        let Some(wanted) = self.runtime_version.as_deref() else {
            // No expected version configured, so nothing to be behind.
            return false;
        };
        if !record.session.capabilities.snapshot {
            // Asked statically rather than found out by trying. A session whose
            // runtime cannot hand its conversation over would fail the capture
            // below on every sweep, forever, and each failure would be logged
            // as though something had gone wrong — when the truthful reading is
            // that this session was never movable and its owner is better
            // served by it simply continuing to work.
            return false;
        }
        record.session.status.wants_runtime()
            && !record.session.status.is_rotating()
            && record.runtime_id.is_some()
            && record.session.runtime_version.as_deref() != Some(wanted)
    }

    /// Move a session onto the current runtime version.
    ///
    /// The sweep's caller, and the reason this module exists. Reports only
    /// whether the move started: nothing is waiting on the answer, and a
    /// session left where it was is a state the next sweep reconsiders.
    pub(crate) async fn rotate(self: &Arc<Self>, record: SessionRecord) -> bool {
        let id = record.session.id.clone();
        let Some(wanted) = self.runtime_version.clone() else {
            return false;
        };
        tracing::info!(
            session_id = %id,
            from = record.session.runtime_version.as_deref().unwrap_or("<unknown>"),
            to = %wanted,
            "moving a session onto a newer runtime"
        );
        self.carry_over(record, Handover::Version(wanted))
            .await
            .is_ok()
    }

    /// Move a live session onto the other plane, because its owner asked.
    ///
    /// The plane is fixed for a session's life in the sense that matters — it
    /// cannot drift, and nothing changes it quietly — but it is not beyond
    /// reach: this is an explicit, owner-initiated act that leaves the session
    /// holding exactly one plane afterwards and records that it happened.
    ///
    /// It is a rotation. The same capture, the same replacement, the same
    /// resume by identifier, and the same `rotating` window in which input is
    /// refused rather than lost; the only difference is that the replacement
    /// is started under the other front-end instead of a newer image.
    ///
    /// Every refusal comes before anything is torn down, and a session that is
    /// refused keeps running exactly as it was. Losing a conversation to a
    /// convenience is the defect `CLAUDE.md` §5 exists to prevent.
    pub async fn change_plane(
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
        mode: SessionMode,
    ) -> Result<Session, CoreError> {
        let record = self.record(id).await?;
        caller.require_own_control_of(&record.session.owner)?;
        if record.session.status.is_terminal() {
            return Err(CoreError::SessionTerminated);
        }
        if record.session.status.is_rotating() {
            // Already mid-handover. Starting a second one would capture a
            // sandbox being torn down and resume from whichever answered last.
            return Err(CoreError::InvalidState);
        }
        if !record.session.status.wants_runtime() || record.runtime_id.is_none() {
            // Nothing to carry. A stopped session is started on whichever
            // plane its profile names when it is revived, and moving one that
            // is not running would mean rotating a sandbox that does not exist.
            return Err(CoreError::InvalidState);
        }
        if record.session.options.mode == mode {
            // Already there. Not an error and not a rotation: charging a
            // session a handover to arrive where it is would be a cost with
            // nothing on the other side of it.
            return Ok(record.session);
        }

        let Some(adapter) = self.adapters.get(&record.session.kernel).cloned() else {
            return Err(CoreError::RuntimeUnavailable(format!(
                "this deployment no longer runs {}",
                record.session.kernel.as_str()
            )));
        };
        // Named before anything is provisioned, exactly as creation does it:
        // a kernel that does not advertise the plane is refused here, with the
        // plane's own name, rather than after its sandbox is gone.
        let capabilities = SessionCapabilities::granted(mode, adapter.capabilities())?;

        if !record.session.capabilities.snapshot {
            // The fail-safe. Both front-ends of the one kernel that serves
            // both planes read each other's conversations — established
            // against the pinned version on #76 — but a kernel that cannot
            // hand its conversation over at all cannot be moved anywhere, and
            // attempting it would trade the conversation for the surface.
            return Err(CoreError::CapabilityUnsupported(
                crate::capabilities::Capability::Snapshot.name().to_string(),
            ));
        }

        tracing::info!(
            session_id = %id,
            from = ?record.session.options.mode,
            to = ?mode,
            "moving a session onto the other plane"
        );
        self.carry_over(record, Handover::Plane { mode, capabilities })
            .await
    }

    /// The handover itself, whatever asked for it.
    ///
    /// Ordered so that the failure cases are survivable. The snapshot is taken
    /// before anything is torn down, so a snapshot that fails costs nothing;
    /// the new runtime is started before the session is declared usable, so a
    /// start that fails leaves the session recoverable by the same path that
    /// recovers any other lost runtime.
    async fn carry_over(
        self: &Arc<Self>,
        record: SessionRecord,
        change: Handover,
    ) -> Result<Session, CoreError> {
        let id = record.session.id.clone();
        let Some(adapter) = self.adapters.get(&record.session.kernel).cloned() else {
            return Err(CoreError::RuntimeUnavailable(format!(
                "this deployment no longer runs {}",
                record.session.kernel.as_str()
            )));
        };
        let Some(old) = record.runtime_id.clone().map(RuntimeHandle) else {
            return Err(CoreError::InvalidState);
        };
        if self.rotation_refused.read().await.contains(&id) {
            // This runtime has already said it cannot hand its conversation
            // over. Asking again every few minutes would achieve nothing and
            // would keep the question in the log forever.
            return Err(CoreError::CapabilityUnsupported(
                crate::capabilities::Capability::Snapshot.name().to_string(),
            ));
        }
        if !self.begin_start(&id).await {
            // Something else already has this session in hand.
            return Err(CoreError::InvalidState);
        }

        // Tried before anything is announced, because capturing changes
        // nothing and failing to capture means the move is off. Announcing
        // first would show every session an "updating" state it then backs out
        // of — and on a runtime that can never be captured, it would do that
        // on every sweep forever.
        let rehearsal = match adapter.snapshot(&old).await {
            Ok(bytes) => bytes,
            Err(e) => {
                // The session cannot be carried across, so it stays exactly
                // where it is. A stale image is bad; losing the conversation
                // is worse. Remembered so this is not retried every few
                // minutes against a runtime that has already said it cannot.
                tracing::warn!(session_id = %id, error = %e, "cannot move this session; leaving it in place");
                self.rotation_refused.write().await.insert(id.clone());
                self.finish_start(&id).await;
                return Err(CoreError::RuntimeUnavailable(format!(
                    "this session's conversation could not be captured, so it has not been moved: {e}"
                )));
            }
        };

        // Announced now. Everything after this point is a handover the owner
        // must not be typing into, and the interface cannot know to stop
        // accepting input until it has been told.
        let Some(rotating) = self.mark_rotating(&record, &change).await else {
            self.finish_start(&id).await;
            return Err(CoreError::RuntimeUnavailable(
                "this session's handover could not be recorded, so it has not been moved".into(),
            ));
        };

        // Taken again, because the first was read before input stopped and
        // anything typed since belongs to the session too. The rehearsal
        // stands in if this one fails: a slightly older conversation is worth
        // far more than none.
        let taken_at = now_rfc3339();
        let restore = match adapter.snapshot(&old).await {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::warn!(session_id = %id, error = %e, "carrying the conversation captured a moment earlier");
                rehearsal
            }
        };

        // Captured to storage as well as handed to the new runtime. The
        // handover is the moment a session is most exposed, and a copy that
        // outlives this process is what makes a failure here recoverable
        // rather than final.
        if let (Some(context), Some(bytes)) = (self.context.clone(), restore.as_ref()) {
            match context.put(&id, bytes.clone()).await {
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(session_id = %id, error = %e, "could not store the snapshot")
                }
            }
        }

        let mut moved = rotating.clone();
        moved.runtime_id = None;
        match &change {
            Handover::Version(wanted) => moved.session.runtime_version = Some(wanted.clone()),
            Handover::Plane { mode, capabilities } => {
                moved.session.options.mode = *mode;
                /*
                 * Re-derived rather than carried. The granted set is the
                 * intersection of what the runtime can do with what the plane
                 * asks for, and the plane has just changed — a set carried
                 * across would advertise a terminal on a session that now
                 * holds a conversation, which is the one thing every interface
                 * downstream trusts cannot happen.
                 */
                moved.session.capabilities = *capabilities;
            }
        }
        moved.session.rotations = moved.session.rotations.saturating_add(1);
        moved.session.rotated_at = Some(taken_at.clone());
        let events = if let Handover::Plane { mode, .. } = &change {
            vec![Event {
                session_id: id.clone(),
                seq: 0,
                kind: EventKind::PlaneChanged {
                    from: record.session.options.mode,
                    to: *mode,
                    snapshot_at: taken_at.clone(),
                },
                appended_at: String::new(),
            }]
        } else {
            Vec::new()
        };
        if let Err(e) = self.persist_durably(&mut moved, events).await {
            tracing::error!(session_id = %id, error = %e, "cannot record the move");
            self.restore_after_failed_move(&record, &old).await;
            self.finish_start(&id).await;
            return Err(CoreError::RuntimeUnavailable(
                "this session's move could not be recorded".into(),
            ));
        }

        /*
         * The attachment goes with the sandbox it was made to, and its holder
         * is told which handover took it: a terminal that ends saying nothing
         * reads as broken, and one that ends claiming the owner opened it
         * elsewhere sends them looking for a tab that does not exist.
         *
         * This happens only after the move is durable. Until that transaction
         * succeeds, the old entry and runtime are the complete rollback path.
         */
        if let Some(entry) = self.entries.write().await.remove(&id) {
            if let Some(attached) = entry.attached.as_ref() {
                let _ = attached.supersede.send(match &change {
                    Handover::Version(_) => crate::lease::Detached::RuntimeReplaced,
                    Handover::Plane { .. } => crate::lease::Detached::PlaneChanged,
                });
            }
        }
        if let Err(e) = adapter.terminate(&old).await {
            tracing::warn!(session_id = %id, error = %e, "could not release the old runtime");
        }

        let geometry = moved.session.geometry;
        let spec = SessionSpec {
            geometry: Some(geometry),
            options: moved.session.options.clone(),
            environment_profile: moved.session.environment_profile.clone(),
            caller_credential: None,
            runtime_credential: None,
            requested_mode: None,
            quoted_rate_card: None,
        };
        let service = Arc::clone(self);
        let session = moved.session.clone();
        tokio::spawn(async move {
            service
                .bring_up(
                    session,
                    spec,
                    adapter,
                    geometry,
                    restore,
                    crate::provision_failure::Abandonment::Keep,
                )
                .await;
        });
        Ok(moved.session)
    }

    /// Put a handover back on its original runtime after the final move failed.
    ///
    /// The old runtime and its entry have not been touched yet. Restoring the
    /// status makes their existing channels usable again; if this compensating
    /// write is also unavailable, leaving both sides `Rotating` is coherent and
    /// recoverable, while tearing anything down would not be.
    async fn restore_after_failed_move(&self, original: &SessionRecord, old: &RuntimeHandle) {
        let id = &original.session.id;
        let mut current = match self.store.get(id).await {
            Ok(Some(record)) => record,
            Ok(None) => return,
            Err(error) => {
                tracing::error!(session_id = %id, %error, "cannot read the failed handover for recovery");
                return;
            }
        };
        if current.session.status != SessionStatus::Rotating
            || current.runtime_id.as_deref() != Some(old.0.as_str())
        {
            return;
        }

        current.session.status = original.session.status;
        current.session.stop_reason = original.session.stop_reason;
        let mut restored = current;
        let event = Event {
            session_id: id.clone(),
            seq: 0,
            kind: EventKind::StatusChanged {
                status: original.session.status,
                stop_reason: original.session.stop_reason,
                detail: Some(
                    "the handover could not be completed; continuing on the existing runtime"
                        .into(),
                ),
                capability: None,
            },
            appended_at: String::new(),
        };
        match self.persist_durably(&mut restored, vec![event]).await {
            Ok(()) => {
                self.mark_entry(id, original.session.status, restored.session.last_seq)
                    .await;
            }
            Err(error) => {
                tracing::error!(
                    session_id = %id,
                    %error,
                    "cannot restore the handover status; leaving the old runtime intact"
                );
            }
        }
    }

    /// Record that a session is mid-handover, and tell anyone watching.
    async fn mark_rotating(
        &self,
        record: &SessionRecord,
        change: &Handover,
    ) -> Option<SessionRecord> {
        let mut rotating = record.clone();
        rotating.session.status = SessionStatus::Rotating;
        rotating.session.stop_reason = None;
        let id = rotating.session.id.clone();
        if let Err(e) = self
            .persist_durably(
                &mut rotating,
                vec![Event {
                    session_id: id.clone(),
                    seq: 0,
                    kind: EventKind::StatusChanged {
                        status: SessionStatus::Rotating,
                        stop_reason: None,
                        detail: Some(change.detail().to_string()),
                        capability: None,
                    },
                    appended_at: String::new(),
                }],
            )
            .await
        {
            tracing::error!(session_id = %id, error = %e, "cannot record the handover");
            return None;
        }
        let seq = rotating.session.last_seq;
        self.mark_entry(&id, SessionStatus::Rotating, seq).await;
        Some(rotating)
    }

    /// Keep a live entry's idea of its own status in step with the record.
    async fn mark_entry(&self, id: &SessionId, status: SessionStatus, seq: u64) {
        if let Some(entry) = self.entries.write().await.get_mut(id) {
            entry.session.status = status;
            entry.session.last_seq = seq;
        }
    }
}
