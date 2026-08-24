//! The live terminal attachment: taking one, and giving it up.
//!
//! Split from `service` so both stay within the project's file-size limit, on
//! a real division: that module owns the session model, which outlives every
//! process, and this owns the attachment, which exists only while somebody is
//! looking at a session.

use std::sync::Arc;

use layer3_adapter::{TermSize, TerminalInput};
use tokio::sync::{broadcast, Mutex, OwnedMutexGuard};

use crate::authz::{Caller, Capability as AuthorizationCapability};
use crate::capabilities::Capability;
use crate::error::CoreError;
use crate::event::{Event, EventKind};
use crate::ids::SessionId;
use crate::log::{append, append_with, drop_undelivered};
use crate::service::{store_failed, Attachment, AttachmentHandle, SessionService};
use crate::store::SessionRecord;
use crate::terminal::{ScreenModel, TerminalGeometry};

type EntryScreen = Arc<Mutex<Box<dyn ScreenModel>>>;

struct RetainedAttachment {
    handle: AttachmentHandle,
    committed: SessionRecord,
}

/// Persist the compensating detach without exposing an outage receiver that can
/// acknowledge and abandon it. The lifecycle guard moves into the detached
/// continuation, so another attach or lifecycle decision cannot overtake it.
fn retain_terminal_detach(
    service: Arc<SessionService>,
    id: SessionId,
    record: SessionRecord,
    screen: EntryScreen,
    lifecycle: OwnedMutexGuard<()>,
) {
    let installed_service = Arc::clone(&service);
    let installed_id = id.clone();
    let installed_screen = Arc::clone(&screen);
    let rejected_service = Arc::clone(&service);
    let rejected_id = id.clone();
    let rejected_screen = Arc::clone(&screen);
    let detach = service.persist_durably_with(
        record,
        vec![Event {
            session_id: id,
            seq: 0,
            kind: EventKind::TerminalDetached,
            appended_at: String::new(),
        }],
        move |committed| async move {
            let mut entries = installed_service.entries.write().await;
            if let Some(entry) = entries
                .get_mut(&installed_id)
                .filter(|entry| Arc::ptr_eq(&entry.screen, &installed_screen))
            {
                entry.session.last_seq = committed.session.last_seq;
                let event_log = Arc::clone(&entry.event_log);
                event_log.synchronize_tail(&mut entry.session.last_seq);
            }
            ((), lifecycle)
        },
        move |_error| async move {
            rejected_service
                .synchronize_live_tail_for(&rejected_id, &rejected_screen)
                .await;
        },
        drop_undelivered,
    );
    drop(detach);
}

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
        caller
            .authorization
            .require(AuthorizationCapability::UseOwnSessions)?;
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
        record.session.capabilities.require(Capability::Terminal)?;
        if !self.entries.read().await.contains_key(id) {
            // The sandbox outlives this process, so a restart loses the
            // attachment and not the runtime. Re-adopting it here is what makes
            // a session survive a deployment: the alternative is a session that
            // reports itself ready, still has a runtime running, and refuses to
            // open — which is a lie the person cannot act on.
            self.readopt(&record).await?;
        }
        let gate = self.lifecycle_gates.gate(id);
        let lifecycle = gate.lock_owned().await;

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

        let commit = {
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
            entry.session.capabilities.require(Capability::Terminal)?;
            let replaced = match (&entry.attached, claim) {
                (Some(_), AttachmentClaim::Exclusive) => return Err(CoreError::AlreadyAttached),
                (Some(held), AttachmentClaim::Takeover) => Some(held.supersede.clone()),
                (None, _) => None,
            };
            let generation = entry.attachments_issued.saturating_add(1);
            let (supersede, superseded) = broadcast::channel(1);
            let screen = Arc::clone(&entry.screen);
            let input = entry.input.clone();
            let live = entry.live.clone();
            let ending = entry.terminal_ending.subscribe();
            let service = Arc::clone(self);
            let session_id = id.clone();
            let rejected_service = Arc::clone(self);
            let rejected_id = id.clone();
            let rejected_screen = Arc::clone(&screen);
            let cleanup_service = Arc::clone(self);
            let cleanup_id = id.clone();
            append_with(
                entry,
                EventKind::TerminalAttached,
                |record| record.session.geometry = geometry,
                move |committed| async move {
                    let installed = {
                        let mut entries = service.entries.write().await;
                        if let Some(entry) = entries
                            .get_mut(&session_id)
                            .filter(|entry| Arc::ptr_eq(&entry.screen, &screen))
                        {
                            entry.session.geometry = committed.session.geometry;
                            entry.session.last_seq = committed.session.last_seq;
                            let event_log = Arc::clone(&entry.event_log);
                            event_log.synchronize_tail(&mut entry.session.last_seq);
                            entry.attachments_issued = generation;
                            entry.attached = Some(Attachment {
                                generation,
                                supersede,
                            });
                            true
                        } else {
                            false
                        }
                    };
                    if !installed {
                        retain_terminal_detach(
                            Arc::clone(&service),
                            session_id.clone(),
                            committed,
                            Arc::clone(&screen),
                            lifecycle,
                        );
                        return (Err(CoreError::SessionNotFound), None);
                    }
                    if let Some(replaced) = replaced {
                        // A send with no receiver means the holder has already
                        // gone, which is the case takeover exists to rescue.
                        let _ = replaced.send(crate::lease::Detached::Superseded);
                        tracing::info!(session_id = %session_id, "the terminal attachment was taken over");
                    }

                    // The relay applies and broadcasts while holding this same
                    // lock, keeping repaint and live output indivisible.
                    let (repaint, output) = {
                        let mut screen = screen.lock().await;
                        let output = match live.as_ref() {
                            Some(live) => live.subscribe(),
                            None => {
                                let (closed, output) = broadcast::channel(1);
                                drop(closed);
                                output
                            }
                        };
                        screen.resize(geometry);
                        (screen.repaint_bytes(), output)
                    };

                    if let Some(input) = input {
                        if input
                            .send(TerminalInput::Resize(TermSize {
                                cols: geometry.columns(),
                                rows: geometry.rows(),
                            }))
                            .await
                            .is_err()
                        {
                            tracing::warn!(session_id = %session_id, "runtime input channel closed");
                        }
                    }

                    let still_installed = service
                        .entries
                        .read()
                        .await
                        .get(&session_id)
                        .is_some_and(|entry| {
                            Arc::ptr_eq(&entry.screen, &screen)
                                && entry
                                    .attached
                                    .as_ref()
                                    .is_some_and(|attached| attached.generation == generation)
                        });
                    if !still_installed {
                        retain_terminal_detach(
                            Arc::clone(&service),
                            session_id.clone(),
                            committed,
                            Arc::clone(&screen),
                            lifecycle,
                        );
                        return (Err(CoreError::SessionNotFound), None);
                    }

                    tracing::info!(session_id = %session_id, cols = geometry.columns(), rows = geometry.rows(), "terminal attached");
                    (
                        Ok(RetainedAttachment {
                            handle: AttachmentHandle {
                                session_id,
                                service,
                                generation,
                                entry_screen: Arc::clone(&screen),
                                release_on_drop: true,
                                output,
                                superseded,
                                ending,
                                repaint,
                                geometry,
                            },
                            committed,
                        }),
                        Some(lifecycle),
                    )
                },
                move |_error| async move {
                    rejected_service
                        .synchronize_live_tail_for(&rejected_id, &rejected_screen)
                        .await;
                },
                move |result: Result<RetainedAttachment, CoreError>, mut ownership| async move {
                    if let Ok(mut retained) = result {
                        let generation = retained.handle.generation;
                        let screen = Arc::clone(&retained.handle.entry_screen);
                        {
                            let mut entries = cleanup_service.entries.write().await;
                            if let Some(entry) = entries.get_mut(&cleanup_id).filter(|entry| {
                                Arc::ptr_eq(&entry.screen, &screen)
                                    && entry
                                        .attached
                                        .as_ref()
                                        .is_some_and(|attached| attached.generation == generation)
                            }) {
                                entry.attached = None;
                            }
                        }
                        retained.handle.release_on_drop = false;
                        drop(retained.handle);
                        if let Some(lifecycle) = ownership.take() {
                            retain_terminal_detach(
                                Arc::clone(&cleanup_service),
                                cleanup_id,
                                retained.committed,
                                screen,
                                lifecycle,
                            );
                        }
                    }
                },
            )
        };
        commit
            .committed()
            .await
            .map_err(store_failed)?
            .map(|retained| retained.handle)
    }

    /// Release the exclusive attachment. Idempotent.
    ///
    /// Scoped to the attachment that is releasing. A superseded client drops
    /// its handle only after its replacement is already live, so an unscoped
    /// release would detach the new holder on the old one's way out — and the
    /// person who just took their own session back would watch it go dead.
    pub async fn release_attachment(&self, id: &SessionId, generation: u64) {
        self.release_attachment_matching(id, generation, None).await;
    }

    pub(crate) async fn release_attachment_from(
        &self,
        id: &SessionId,
        generation: u64,
        screen: &EntryScreen,
    ) {
        self.release_attachment_matching(id, generation, Some(screen))
            .await;
    }

    async fn release_attachment_matching(
        &self,
        id: &SessionId,
        generation: u64,
        screen: Option<&EntryScreen>,
    ) {
        let mut entries = self.entries.write().await;
        if let Some(entry) = entries.get_mut(id) {
            if screen.is_none_or(|screen| Arc::ptr_eq(&entry.screen, screen))
                && entry
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
