//! The live terminal attachment: taking one, and giving it up.
//!
//! Split from `service` so both stay within the project's file-size limit, on
//! a real division: that module owns the session model, which outlives every
//! process, and this owns the attachment, which exists only while somebody is
//! looking at a session.

use std::sync::{Arc, Weak};

use layer3_adapter::{TermSize, TerminalInput, TerminalInputSender};
use tokio::sync::{broadcast, watch, Mutex, OwnedMutexGuard};

use crate::authz::{Caller, Capability as AuthorizationCapability};
use crate::capabilities::Capability;
use crate::error::CoreError;
use crate::event::{Event, EventKind};
use crate::ids::{SessionId, SubjectId};
use crate::log::{append, append_with, CleanupAttempt, DeferredCleanup, RestartableCommit};
use crate::provision_support::RetainedRuntimeInteraction;
use crate::service::{store_failed, Attachment, AttachmentHandle, SessionEntry, SessionService};
use crate::store::{SessionRecord, StoreError};
use crate::terminal::{ScreenModel, TerminalGeometry};

type EntryScreen = Arc<Mutex<Box<dyn ScreenModel>>>;

struct RetainedAttachment {
    handle: AttachmentHandle,
    committed: SessionRecord,
    ownership: AttachmentInteractionOwnership,
}

struct AttachmentInteractionOwnership {
    lifecycle: Option<OwnedMutexGuard<()>>,
    interaction: RetainedRuntimeInteraction,
}

impl AttachmentInteractionOwnership {
    fn release(self) {
        drop(self.lifecycle);
        drop(self.interaction);
    }
}

/// An unread successful attach handed back to service-owned recovery.
struct DeferredAttachmentCleanup {
    service: Weak<SessionService>,
    id: SessionId,
    generation: Option<u64>,
    screen: EntryScreen,
    committed: Option<SessionRecord>,
    commit: Option<RestartableCommit>,
    outcome: Option<Result<SessionRecord, StoreError>>,
    ownership: Option<AttachmentInteractionOwnership>,
}

impl DeferredCleanup for DeferredAttachmentCleanup {
    fn session_id(&self) -> &SessionId {
        &self.id
    }

    fn attempt(&mut self) -> CleanupAttempt<'_> {
        Box::pin(async move {
            let Some(service) = self.service.upgrade() else {
                self.committed.take();
                if let Some(ownership) = self.ownership.take() {
                    ownership.release();
                }
                return;
            };
            if self.commit.is_none() {
                let mut entries = service.entries.write().await;
                if let Some(generation) = self.generation {
                    if let Some(entry) = entries.get_mut(&self.id).filter(|entry| {
                        Arc::ptr_eq(&entry.screen, &self.screen)
                            && entry
                                .attached
                                .as_ref()
                                .is_some_and(|attached| attached.generation == generation)
                    }) {
                        entry.attached = None;
                    }
                }
                drop(entries);
                let Some(committed) = self.committed.take() else {
                    return;
                };
                self.commit = Some(service.retain_cleanup_durably(
                    committed,
                    vec![Event {
                        session_id: self.id.clone(),
                        seq: 0,
                        kind: EventKind::TerminalDetached,
                        appended_at: String::new(),
                    }],
                ));
            }
            let commit = self
                .commit
                .as_mut()
                .expect("a deferred detach retains its durable write");
            commit.restart();
            if self.outcome.is_none() {
                self.outcome = Some(commit.committed().await);
            }
            match self
                .outcome
                .as_ref()
                .expect("a completed cleanup write retains its outcome")
            {
                Ok(committed) => {
                    let mut entries = service.entries.write().await;
                    if let Some(entry) = entries
                        .get_mut(&self.id)
                        .filter(|entry| Arc::ptr_eq(&entry.screen, &self.screen))
                    {
                        entry.session.last_seq = committed.session.last_seq;
                        let event_log = Arc::clone(&entry.event_log);
                        event_log.synchronize_tail(&mut entry.session.last_seq);
                    }
                }
                Err(error) => {
                    service
                        .synchronize_live_tail_for(&self.id, &self.screen)
                        .await;
                    tracing::error!(session_id = %self.id, %error, "retained terminal detach failed");
                }
            }
            if let Some(ownership) = self.ownership.take() {
                ownership.release();
            }
            self.commit.take();
            self.outcome.take();
        })
    }
}

/// Persist the compensating detach without exposing an outage receiver that can
/// acknowledge and abandon it. The lifecycle guard remains in the service-owned
/// restartable job, so another attach or lifecycle decision cannot overtake it.
fn retain_terminal_detach(
    service: Arc<SessionService>,
    id: SessionId,
    record: SessionRecord,
    screen: EntryScreen,
    ownership: AttachmentInteractionOwnership,
) {
    service
        .event_logs
        .retain_cleanup(DeferredAttachmentCleanup {
            service: Arc::downgrade(&service),
            id,
            generation: None,
            screen,
            committed: Some(record),
            commit: None,
            outcome: None,
            ownership: Some(ownership),
        });
}

fn retain_unread_attachment(
    service: Arc<SessionService>,
    id: SessionId,
    retained: RetainedAttachment,
) {
    let RetainedAttachment {
        mut handle,
        committed,
        ownership,
    } = retained;
    let generation = handle.generation;
    let screen = Arc::clone(&handle.entry_screen);
    handle.release_on_drop = false;
    drop(handle);
    service
        .event_logs
        .retain_cleanup(DeferredAttachmentCleanup {
            service: Arc::downgrade(&service),
            id,
            generation: Some(generation),
            screen,
            committed: Some(committed),
            commit: None,
            outcome: None,
            ownership: Some(ownership),
        });
}

fn refuse_committed_attachment(
    service: Arc<SessionService>,
    id: SessionId,
    committed: SessionRecord,
    screen: EntryScreen,
    ownership: AttachmentInteractionOwnership,
    error: CoreError,
) -> (Result<RetainedAttachment, CoreError>, ()) {
    retain_terminal_detach(service, id, committed, screen, ownership);
    (Err(error), ())
}

fn validate_attachment_link(
    entry: &SessionEntry,
    owner: &SubjectId,
    screen: &EntryScreen,
    runtime_generation: u64,
    input: &Option<TerminalInputSender>,
    live: &Option<broadcast::Sender<Vec<u8>>>,
    ending: &watch::Sender<Option<crate::lease::Detached>>,
    claim: AttachmentClaim,
) -> Result<(), CoreError> {
    if !entry.session.is_owned_by(owner) {
        return Err(CoreError::Forbidden);
    }
    if entry.session.status.is_terminal() {
        return Err(CoreError::SessionTerminated);
    }
    if entry.session.status.is_rotating() {
        return Err(CoreError::SessionRotating);
    }
    entry.session.capabilities.require(Capability::Terminal)?;
    let same_input = match (&entry.input, input) {
        (Some(current), Some(expected)) => current.same_channel(expected),
        (None, None) => true,
        _ => false,
    };
    let same_live = match (&entry.live, live) {
        (Some(current), Some(expected)) => current.same_channel(expected),
        (None, None) => true,
        _ => false,
    };
    if !Arc::ptr_eq(&entry.screen, screen)
        || entry.runtime_generation != runtime_generation
        || !same_input
        || !same_live
        || !entry.terminal_ending.same_channel(ending)
    {
        return Err(CoreError::RuntimeUnavailable(
            "the live terminal link changed before attachment delivery".into(),
        ));
    }
    if entry.attached.is_some() && claim == AttachmentClaim::Exclusive {
        return Err(CoreError::AlreadyAttached);
    }
    Ok(())
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
        if record.session.status.is_rotating() {
            return Err(CoreError::SessionRotating);
        }
        let entries = self.entries.read().await;
        let live_is_rotating = entries
            .get(id)
            .is_some_and(|entry| entry.session.status.is_rotating());
        let missing_live_entry = !entries.contains_key(id);
        drop(entries);
        if live_is_rotating {
            return Err(CoreError::SessionRotating);
        }
        if missing_live_entry {
            let rotation_in_progress = self.rotation_in_progress(id).await;
            if rotation_in_progress {
                return Err(CoreError::SessionRotating);
            }
        }
        record.session.capabilities.require(Capability::Terminal)?;
        if missing_live_entry {
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

        // Readoption and credential supply may each cross a lifecycle boundary.
        // Interact must name the exact durable record after both have settled.
        let record = self.record(id).await?;
        if !record.session.is_owned_by(&caller.subject) {
            return Err(CoreError::Forbidden);
        }
        if record.session.status.is_terminal() {
            return Err(CoreError::SessionTerminated);
        }
        if record.session.status.is_rotating() {
            return Err(CoreError::SessionRotating);
        }
        record.session.capabilities.require(Capability::Terminal)?;

        let service = Arc::clone(self);
        let owner = caller.subject.clone();
        let session_id = id.clone();
        let undelivered_service = Arc::clone(self);
        let undelivered_id = id.clone();
        let retained = self
            .run_retained_runtime_interaction_at(
                &record,
                move |authority, interaction| async move {
                    let lifecycle = if authority.is_durably_fenced() {
                        None
                    } else {
                        Some(service.lock_lifecycle(&session_id).await)
                    };
                    let input_fence = service
                        .input_gates
                        .gate(&session_id)
                        .read_owned()
                        .await;
        let commit = {
            let mut entries = service.entries.write().await;
            let entry = entries.get_mut(&session_id).ok_or_else(|| {
                CoreError::RuntimeUnavailable("this session has no live terminal entry".into())
            })?;
            // Attaching is acting, not observing, so ownership is required
            // even for a privileged reader.
            if !entry.session.is_owned_by(&owner) {
                return Err(CoreError::Forbidden);
            }
            if entry.session.status.is_terminal() {
                return Err(CoreError::SessionTerminated);
            }
            // The durable check above can race a handover while this operation
            // waits for lifecycle ownership. Never attach a client to the
            // runtime that handover is about to supersede.
            if entry.session.status.is_rotating() {
                return Err(CoreError::SessionRotating);
            }
            entry.session.capabilities.require(Capability::Terminal)?;
            if entry.attached.is_some() && claim == AttachmentClaim::Exclusive {
                return Err(CoreError::AlreadyAttached);
            }
            let boundary = authority.ensure_delivery_allowed()?;
            if entry.runtime_generation != boundary.authority.generation {
                return Err(CoreError::RuntimeUnavailable(
                    "the live terminal belongs to another runtime generation".into(),
                ));
            }
            let runtime_generation = entry.runtime_generation;
            let screen = Arc::clone(&entry.screen);
            let input = entry.input.clone();
            let live = entry.live.clone();
            let ending = entry.terminal_ending.clone();
            let rejected_service = Arc::clone(&service);
            let rejected_id = session_id.clone();
            let rejected_screen = Arc::clone(&screen);
            let cleanup_service = Arc::clone(&service);
            let cleanup_id = session_id.clone();
            let continuation_service = Arc::clone(&service);
            append_with(
                entry,
                EventKind::TerminalAttached,
                |record| record.session.geometry = geometry,
                move |committed| async move {
                    let service = continuation_service;
                    let resumed = interaction.resume_before_delivery().await;
                    let ownership = AttachmentInteractionOwnership {
                        lifecycle,
                        interaction,
                    };
                    let delivery_authority = match resumed {
                        Ok(authority) => authority,
                        Err(error) => {
                            return refuse_committed_attachment(
                                Arc::clone(&service),
                                session_id.clone(),
                                committed,
                                Arc::clone(&screen),
                                ownership,
                                error,
                            );
                        }
                    };

                    // Keep the repaint/live boundary closed while the exact
                    // runtime resize is admitted and settles. Runtime output
                    // takes this same screen lock before broadcasting a frame.
                    let mut screen_guard = screen.lock().await;

                    let validation = service
                        .entries
                        .read()
                        .await
                        .get(&session_id)
                        .ok_or_else(|| {
                            CoreError::RuntimeUnavailable(
                                "this session has no live terminal entry".into(),
                            )
                        })
                        .and_then(|entry| {
                            validate_attachment_link(
                                entry,
                                &owner,
                                &screen,
                                runtime_generation,
                                &input,
                                &live,
                                &ending,
                                claim,
                            )
                        });
                    if let Err(error) = validation {
                        return refuse_committed_attachment(
                            Arc::clone(&service),
                            session_id.clone(),
                            committed,
                            Arc::clone(&screen),
                            ownership,
                            error,
                        );
                    }

                    if let Err(error) = delivery_authority.ensure_delivery_allowed() {
                        return refuse_committed_attachment(
                            Arc::clone(&service),
                            session_id.clone(),
                            committed,
                            Arc::clone(&screen),
                            ownership,
                            error,
                        );
                    }
                    if let Some(input) = input.as_ref() {
                        if let Err(error) = service
                            .forward_input(
                                &session_id,
                                input,
                                TerminalInput::Resize(TermSize {
                                    cols: geometry.columns(),
                                    rows: geometry.rows(),
                                }),
                            )
                            .await
                        {
                            return refuse_committed_attachment(
                                Arc::clone(&service),
                                session_id.clone(),
                                committed,
                                Arc::clone(&screen),
                                ownership,
                                error,
                            );
                        }
                    }

                    // A successful receipt makes publication mandatory. Do not
                    // recheck an expiring authority between it and local state.
                    let installed = {
                        let mut entries = service.entries.write().await;
                        let entry = match entries.get_mut(&session_id) {
                            Some(entry) => entry,
                            None => {
                                return refuse_committed_attachment(
                                    Arc::clone(&service),
                                    session_id.clone(),
                                    committed,
                                    Arc::clone(&screen),
                                    ownership,
                                    CoreError::RuntimeUnavailable(
                                        "the terminal entry changed after resize delivery".into(),
                                    ),
                                );
                            }
                        };
                        if let Err(error) = validate_attachment_link(
                            entry,
                            &owner,
                            &screen,
                            runtime_generation,
                            &input,
                            &live,
                            &ending,
                            claim,
                        ) {
                            return refuse_committed_attachment(
                                Arc::clone(&service),
                                session_id.clone(),
                                committed,
                                Arc::clone(&screen),
                                ownership,
                                error,
                            );
                        }
                        let replaced = entry
                            .attached
                            .as_ref()
                            .map(|held| held.supersede.clone());
                        let generation = entry.attachments_issued.saturating_add(1);
                        let (supersede, superseded) = broadcast::channel(1);
                        entry.session.geometry = committed.session.geometry;
                        entry.session.last_seq = committed.session.last_seq;
                        let event_log = Arc::clone(&entry.event_log);
                        event_log.synchronize_tail(&mut entry.session.last_seq);
                        entry.attachments_issued = generation;
                        entry.attached = Some(Attachment {
                            generation,
                            supersede,
                        });
                        (replaced, generation, superseded)
                    };
                    let (replaced, generation, superseded) = installed;

                    let output = match live.as_ref() {
                        Some(live) => live.subscribe(),
                        None => {
                            let (closed, output) = broadcast::channel(1);
                            drop(closed);
                            output
                        }
                    };
                    screen_guard.resize(geometry);
                    let repaint = screen_guard.repaint_bytes();
                    drop(screen_guard);
                    drop(input_fence);

                    if let Some(replaced) = replaced {
                        // A send with no receiver means the holder has already
                        // gone, which is the case takeover exists to rescue.
                        let _ = replaced.send(crate::lease::Detached::Superseded);
                        tracing::info!(session_id = %session_id, "the terminal attachment was taken over");
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
                                ending: ending.subscribe(),
                                repaint,
                                geometry,
                            },
                            committed,
                            ownership,
                        }),
                        (),
                    )
                },
                move |_error| async move {
                    rejected_service
                        .synchronize_ordered_live_tail_for(&rejected_id, &rejected_screen)
                        .await;
                },
                move |result: Result<RetainedAttachment, CoreError>, ()| {
                    if let Ok(retained) = result {
                        retain_unread_attachment(cleanup_service, cleanup_id, retained);
                    }
                },
            )
        };
                    commit.committed().await.map_err(store_failed)?
                },
                move |_reason, result, _interaction| {
                    if let Ok(retained) = result {
                        retain_unread_attachment(
                            undelivered_service,
                            undelivered_id,
                            retained,
                        );
                    }
                },
            )
            .await?;
        let RetainedAttachment {
            handle, ownership, ..
        } = retained;
        ownership.release();
        Ok(handle)
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
