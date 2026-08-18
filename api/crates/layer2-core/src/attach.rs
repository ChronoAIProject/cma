//! The live terminal attachment: taking one, and giving it up.
//!
//! Split from `service` so both stay within the project's file-size limit, on
//! a real division: that module owns the session model, which outlives every
//! process, and this owns the attachment, which exists only while somebody is
//! looking at a session.

use std::sync::Arc;

use layer3_adapter::{TermSize, TerminalInput};
use tokio::sync::broadcast;

use crate::authz::{Caller, Capability};
use crate::error::CoreError;
use crate::event::EventKind;
use crate::ids::SessionId;
use crate::service::{append, Attachment, AttachmentHandle, SessionService};
use crate::terminal::TerminalGeometry;

/// How an attach should treat an attachment somebody already holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AttachmentClaim {
    /// Refuse when the terminal is already held. The default, and the right
    /// answer for anything that might be a second tab opened by accident.
    #[default]
    Exclusive,
    /// Take it from whoever has it, and tell them so.
    ///
    /// Only the owner may do this, and only deliberately. It exists because
    /// the alternative — a client whose connection died in a way the server
    /// has not noticed yet, holding an attachment its owner cannot reclaim —
    /// leaves somebody locked out of their own session with no way back in.
    Takeover,
}

impl SessionService {
    /// Take the exclusive terminal attachment.
    ///
    /// The geometry is applied to the screen before the repaint is produced, so
    /// the caller's first paint is already at its own size.
    pub async fn attach_terminal(
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
        geometry: TerminalGeometry,
        claim: AttachmentClaim,
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
            // The sandbox outlives this process, so a restart loses the
            // attachment and not the runtime. Re-adopting it here is what makes
            // a session survive a deployment: the alternative is a session that
            // reports itself ready, still has a runtime running, and refuses to
            // open — which is a lie the person cannot act on.
            self.readopt(&record).await?;
        }

        // The session may have been recovered before its owner had signed back
        // in, when there was no credential to be had. This is the moment they
        // are demonstrably here, so it is the moment to check: without it the
        // session opens, looks healthy, and refuses every model call until a
        // sweep happens to notice minutes later.
        let unfunded = self
            .entries
            .read()
            .await
            .get(id)
            .is_some_and(|e| e.subject_credential.is_none());
        if unfunded {
            self.supply_credential(id, &caller.subject).await;
        }

        let (screen, live, generation, superseded) = {
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
            match (&entry.attached, claim) {
                (Some(_), AttachmentClaim::Exclusive) => return Err(CoreError::AlreadyAttached),
                (Some(held), AttachmentClaim::Takeover) => {
                    // A send with no receiver means the holder has already
                    // gone, which is not a failure — it is the case takeover
                    // exists to rescue.
                    let _ = held.supersede.send(crate::lease::Detached::Superseded);
                    tracing::info!(session_id = %id, "the terminal attachment was taken over");
                }
                (None, _) => {}
            }
            entry.attachments_issued += 1;
            let generation = entry.attachments_issued;
            let (supersede, superseded) = broadcast::channel(1);
            entry.attached = Some(Attachment {
                generation,
                supersede,
            });
            entry.session.geometry = geometry;
            append(entry, EventKind::TerminalAttached);
            (
                Arc::clone(&entry.screen),
                entry.live.clone(),
                generation,
                superseded,
            )
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
            generation,
            output,
            superseded,
            repaint,
            geometry,
        })
    }

    /// Release the exclusive attachment. Idempotent.
    ///
    /// Scoped to the attachment that is releasing. A superseded client drops
    /// its handle only after its replacement is already live, so an unscoped
    /// release would detach the new holder on the old one's way out — and the
    /// person who just took their own session back would watch it go dead.
    pub async fn release_attachment(&self, id: &SessionId, generation: u64) {
        let mut entries = self.entries.write().await;
        if let Some(entry) = entries.get_mut(id) {
            if entry
                .attached
                .as_ref()
                .is_some_and(|a| a.generation == generation)
            {
                entry.attached = None;
                append(entry, EventKind::TerminalDetached);
                tracing::info!(session_id = %id, "terminal detached");
            }
        }
        drop(entries);
        self.flush(id).await;
    }
}
