//! Carrying a session onto a newer runtime without interrupting it.
//!
//! The runtime image changes — a new agent version, a patched base, a fixed
//! bug — and sessions started on the old one would otherwise stay there until
//! their owners happened to stop them. Upgrading a sandbox in place is not an
//! option: the image is the sandbox.
//!
//! So the session is moved instead. Its agent's conversation is captured, the
//! old sandbox is released, a new one is started on the current image, and the
//! agent resumes from the capture. The session identifier, its history, its
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

use layer3_adapter::{AdapterError, RuntimeHandle};

use crate::clock::now_rfc3339;
use crate::event::{Event, EventKind};
use crate::ids::SessionId;
use crate::service::SessionService;
use crate::session::{SessionSpec, SessionStatus};
use crate::store::SessionRecord;

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
        record.session.status.wants_runtime()
            && !record.session.status.is_rotating()
            && record.runtime_id.is_some()
            && record.session.runtime_version.as_deref() != Some(wanted)
    }

    /// Move a session onto the current runtime version.
    ///
    /// Ordered so that the failure cases are survivable. The snapshot is taken
    /// before anything is torn down, so a snapshot that fails costs nothing;
    /// the new runtime is started before the session is declared usable, so a
    /// start that fails leaves the session recoverable by the same path that
    /// recovers any other lost runtime.
    pub(crate) async fn rotate(self: &Arc<Self>, record: SessionRecord) -> bool {
        let id = record.session.id.clone();
        let Some(wanted) = self.runtime_version.clone() else {
            return false;
        };
        let Some(adapter) = self.adapters.get(&record.session.kernel).cloned() else {
            return false;
        };
        let Some(old) = record.runtime_id.clone().map(RuntimeHandle) else {
            return false;
        };
        if !self.begin_start(&id).await {
            // Something else already has this session in hand.
            return false;
        }

        tracing::info!(
            session_id = %id,
            from = record.session.runtime_version.as_deref().unwrap_or("<unknown>"),
            to = %wanted,
            "moving a session onto a newer runtime"
        );

        // Announced first. Everything after this point is a handover the owner
        // must not be typing into, and the interface cannot know to stop
        // accepting input until it has been told.
        let Some(rotating) = self.mark_rotating(&record).await else {
            self.finish_start(&id).await;
            return false;
        };

        let taken_at = now_rfc3339();
        let restore = match adapter.snapshot(&old).await {
            Ok(bytes) => bytes,
            Err(AdapterError::Unsupported(reason)) => {
                // The session cannot be carried across, so it is left exactly
                // where it is. Running it on an old image indefinitely is a
                // worse outcome than a stale image, but losing the
                // conversation is worse than both.
                tracing::warn!(session_id = %id, reason, "cannot move this session; leaving it in place");
                self.restore_status(&rotating).await;
                self.finish_start(&id).await;
                return false;
            }
            Err(e) => {
                tracing::warn!(session_id = %id, error = %e, "could not capture the session; leaving it in place");
                self.restore_status(&rotating).await;
                self.finish_start(&id).await;
                return false;
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

        // The attachment goes with the sandbox it was made to.
        self.entries.write().await.remove(&id);
        if let Err(e) = adapter.terminate(&old).await {
            tracing::warn!(session_id = %id, error = %e, "could not release the old runtime");
        }

        let mut moved = rotating.clone();
        moved.runtime_id = None;
        moved.session.runtime_version = Some(wanted);
        moved.session.rotations = moved.session.rotations.saturating_add(1);
        moved.session.rotated_at = Some(taken_at);
        moved.session.last_seq += 1;
        if let Err(e) = self.store.upsert(&moved).await {
            tracing::error!(session_id = %id, error = %e, "cannot record the move");
            self.finish_start(&id).await;
            return false;
        }

        let geometry = moved.session.geometry;
        let spec = SessionSpec {
            kernel: moved.session.kernel.clone(),
            geometry: Some(geometry),
            options: moved.session.options.clone(),
            subject_credential: None,
        };
        let service = Arc::clone(self);
        let session = moved.session.clone();
        tokio::spawn(async move {
            service
                .bring_up(session, spec, adapter, geometry, restore)
                .await;
        });
        true
    }

    /// Record that a session is mid-handover, and tell anyone watching.
    async fn mark_rotating(&self, record: &SessionRecord) -> Option<SessionRecord> {
        let mut rotating = record.clone();
        rotating.session.status = SessionStatus::Rotating;
        rotating.session.stop_reason = None;
        rotating.session.last_seq += 1;
        let seq = rotating.session.last_seq;
        let id = rotating.session.id.clone();

        if let Err(e) = self.store.upsert(&rotating).await {
            tracing::error!(session_id = %id, error = %e, "cannot record the handover");
            return None;
        }
        if let Err(e) = self
            .store
            .append_events(&[Event {
                session_id: id.clone(),
                seq,
                kind: EventKind::StatusChanged {
                    status: SessionStatus::Rotating,
                    stop_reason: None,
                    detail: Some(
                        "moving this session to an updated runtime; it will come back where it left off"
                            .to_string(),
                    ),
                },
            }])
            .await
        {
            tracing::warn!(session_id = %id, error = %e, "cannot announce the handover");
        }
        self.mark_entry(&id, SessionStatus::Rotating, seq).await;
        Some(rotating)
    }

    /// Put a session back the way it was, after a handover that never started.
    ///
    /// Nothing was torn down, so the session is exactly as usable as it was a
    /// moment ago — and leaving it marked as rotating would make a healthy
    /// session permanently refuse input.
    async fn restore_status(&self, rotating: &SessionRecord) {
        let mut back = rotating.clone();
        back.session.status = SessionStatus::Ready;
        back.session.last_seq += 1;
        let seq = back.session.last_seq;
        let id = back.session.id.clone();
        if let Err(e) = self.store.upsert(&back).await {
            tracing::error!(session_id = %id, error = %e, "cannot undo the handover marker");
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
            tracing::warn!(session_id = %id, error = %e, "cannot announce the undo");
        }
        self.mark_entry(&id, SessionStatus::Ready, seq).await;
    }

    /// Keep a live entry's idea of its own status in step with the record.
    async fn mark_entry(&self, id: &SessionId, status: SessionStatus, seq: u64) {
        if let Some(entry) = self.entries.write().await.get_mut(id) {
            entry.session.status = status;
            entry.session.last_seq = seq;
        }
    }
}
