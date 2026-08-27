//! Driving a live session's terminal.
//!
//! Split from the service so that file is about session state and this one is
//! about the bytes and geometry flowing to a running runtime. Every method here
//! requires ownership: driving a terminal is acting, not observing, and a
//! privileged reader deliberately holds no capability that reaches this far.

use std::sync::Arc;

use layer3_adapter::{TermSize, TerminalInput, TerminalInputSender, TerminalOutput};
use tokio::sync::{broadcast, mpsc, watch, Mutex};

use crate::authz::{Caller, Capability as AuthorizationCapability};
use crate::capabilities::Capability;
use crate::error::CoreError;
use crate::event::EventKind;
use crate::ids::{SessionId, SubjectId};
use crate::log::{append_with, drop_undelivered};
use crate::provision_support::RuntimeInteractionAuthority;
use crate::service::{store_failed, SessionEntry, SessionService};
use crate::store::SessionRecord;
use crate::terminal::{ScreenModel, ScreenSnapshot, TerminalGeometry};

struct TerminalResizeTarget<'a> {
    runtime_id: Option<&'a str>,
    runtime_generation: u64,
    screen: &'a Arc<Mutex<Box<dyn ScreenModel>>>,
    input: Option<&'a TerminalInputSender>,
}

/// Pump runtime output into the screen model and out to any live subscriber.
pub(crate) fn spawn_relay(
    service: Arc<SessionService>,
    id: SessionId,
    mut output: mpsc::Receiver<TerminalOutput>,
    screen: Arc<Mutex<Box<dyn ScreenModel>>>,
    live: broadcast::Sender<Vec<u8>>,
    ending: watch::Sender<Option<crate::lease::Detached>>,
) {
    tokio::spawn(async move {
        let reported = loop {
            match output.recv().await {
                Some(TerminalOutput::Bytes(chunk)) => {
                    let mut screen = screen.lock().await;
                    screen.apply(&chunk);
                    // A send with no subscribers is not an error: the session is
                    // simply detached, and the screen has absorbed the bytes.
                    // Keep the same guard through the send: an attachment takes
                    // this lock before subscribing and repainting, making the
                    // snapshot/live boundary indivisible.
                    let _ = live.send(chunk);
                }
                Some(TerminalOutput::Exited { code }) => {
                    break Some(crate::lease::Detached::ProcessExited { code });
                }
                // The link ended without a process report. Preserve that
                // distinction instead of manufacturing a successful exit.
                None => break None,
            }
        };

        if let Some(reported) = reported {
            // Recorded before closing the byte stream. A receiver observes all
            // queued output, then Closed, then reads this exact ending.
            ending.send_replace(Some(reported));
        }

        // Close only the stream belonging to this relay. An older relay can end
        // after a rotation has already installed its replacement under the same
        // session id.
        let mut entries = service.entries.write().await;
        if let Some(entry) = entries.get_mut(&id) {
            if entry
                .live
                .as_ref()
                .is_some_and(|current| current.same_channel(&live))
            {
                entry.input = None;
                entry.live = None;
            }
        }
        drop(entries);
        drop(live);
        tracing::info!(session_id = %id, "runtime output ended");
    });
}

impl SessionService {
    /// Deliver input to the session's terminal.
    ///
    /// Unguarded against a live attachment on purpose: the client holding one
    /// drives the terminal through this very call, so refusing here would
    /// refuse the attachment its own keystrokes. Whether a caller that is *not*
    /// the holder may write is a question about that caller, and is asked where
    /// the caller is known — see [`Self::terminal_is_held`].
    pub async fn send_input(
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
        bytes: Vec<u8>,
    ) -> Result<(), CoreError> {
        self.deliver_terminal_input(caller, id, TerminalInput::Bytes(bytes))
            .await
    }
    /// Send a named, bounded terminal control request to the runtime.
    ///
    /// Allowed while somebody is attached, unlike free-form input. The runtime
    /// translates a documented name into one control byte, which is the bounded
    /// action an owner may need from elsewhere to stop a runaway command while
    /// a screen is held open.
    pub async fn send_signal(
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
        signal: String,
    ) -> Result<(), CoreError> {
        self.deliver_terminal_input(caller, id, TerminalInput::Signal(signal))
            .await
    }

    /// Deliver one receipt-only mutation to the exact authoritative terminal.
    async fn deliver_terminal_input(
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
        input: TerminalInput,
    ) -> Result<(), CoreError> {
        let record = self.terminal_interaction_record(caller, id, true).await?;
        let service = Arc::clone(self);
        let owner = caller.subject.clone();
        let id = id.clone();
        self.run_runtime_interaction_at(&record, move |authority| async move {
            // Compatibility lifecycle operations have only process-local
            // exclusion and take this gate before the input fence. Coordinated
            // Interact already excludes Lifecycle at the store boundary.
            let _lifecycle = if authority.is_durably_fenced() {
                None
            } else {
                Some(service.lock_lifecycle(&id).await)
            };
            let _input_fence = service.input_gates.gate(&id).read_owned().await;
            let (runtime_id, runtime_generation, sender) = {
                let entries = service.entries.read().await;
                let entry = entries.get(&id).ok_or_else(|| {
                    CoreError::RuntimeUnavailable("this session has no live terminal entry".into())
                })?;
                if !entry.session.is_owned_by(&owner) {
                    return Err(CoreError::Forbidden);
                }
                if entry.session.status.is_terminal() {
                    return Err(CoreError::SessionTerminated);
                }
                if entry.session.status.is_rotating() {
                    return Err(CoreError::SessionRotating);
                }
                entry.session.capabilities.require(Capability::Terminal)?;
                let sender = entry.input.clone().ok_or_else(|| {
                    CoreError::RuntimeUnavailable(
                        "this session holds no terminal input sender".into(),
                    )
                })?;
                (
                    entry.runtime.as_ref().map(|runtime| runtime.0.clone()),
                    entry.runtime_generation,
                    sender,
                )
            };

            authority.ensure_runtime_delivery_allowed(runtime_id.as_deref(), runtime_generation)?;
            service.forward_input(&id, &sender, input).await
        })
        .await
    }

    /// Authorize and rebuild local runtime state before interaction acquisition.
    async fn terminal_interaction_record(
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
        reserve_initial_prompt: bool,
    ) -> Result<SessionRecord, CoreError> {
        caller
            .authorization
            .require(AuthorizationCapability::UseOwnSessions)?;
        let record = self.record(id).await?;
        Self::require_terminal_interaction_record(caller, &record, reserve_initial_prompt)?;

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
            if self.rotation_in_progress(id).await {
                return Err(CoreError::SessionRotating);
            }
            self.readopt(&record).await?;
        }

        // Readoption may settle capabilities or advance readiness. Interaction
        // acquisition must name the exact durable boundary after that work.
        let record = self.record(id).await?;
        Self::require_terminal_interaction_record(caller, &record, reserve_initial_prompt)?;
        Ok(record)
    }

    fn require_terminal_interaction_record(
        caller: &Caller,
        record: &SessionRecord,
        reserve_initial_prompt: bool,
    ) -> Result<(), CoreError> {
        if !record.session.is_owned_by(&caller.subject) {
            return Err(CoreError::Forbidden);
        }
        if reserve_initial_prompt {
            if let Some(state) = record.reserved_initial_prompt_state() {
                return Err(CoreError::InitialPromptReserved(state));
            }
        }
        if record.session.status.is_terminal() {
            return Err(CoreError::SessionTerminated);
        }
        if record.is_effectively_rotating() {
            return Err(CoreError::SessionRotating);
        }
        if !record.session.capabilities.unrecorded() {
            record.session.capabilities.require(Capability::Terminal)?;
        }
        Ok(())
    }
    /// Change the session's terminal geometry.
    ///
    /// Unguarded here for the same reason as input: the attached client
    /// declares its own geometry through this call. Once the geometry is
    /// durable it is the desired state for this runtime or its successor. A
    /// fenced or rejected runtime delivery therefore leaves that durable state
    /// for retry or readoption to apply instead of rolling it back.
    pub async fn resize_terminal(
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
        geometry: TerminalGeometry,
    ) -> Result<(), CoreError> {
        let record = self.terminal_interaction_record(caller, id, false).await?;
        let service = Arc::clone(self);
        let owner = caller.subject.clone();
        let id = id.clone();
        self.run_retained_runtime_interaction_at(
            &record,
            move |authority, interaction| async move {
                // Compatibility lifecycle operations have only process-local
                // exclusion and take this gate before the input fence. A
                // coordinated Interact lease excludes Lifecycle durably.
                let lifecycle = if authority.is_durably_fenced() {
                    None
                } else {
                    Some(service.lock_lifecycle(&id).await)
                };
                let input_fence = service.input_gates.gate(&id).read_owned().await;
                let commit = {
                    let mut entries = service.entries.write().await;
                    let entry = entries.get_mut(&id).ok_or_else(|| {
                        CoreError::RuntimeUnavailable(
                            "this session has no live terminal entry".into(),
                        )
                    })?;
                    Self::require_resize_entry(entry, &owner)?;
                    let runtime_id = entry.runtime.as_ref().map(|runtime| runtime.0.clone());
                    let runtime_generation = entry.runtime_generation;
                    // Committing desired geometry is itself a runtime-scoped
                    // mutation. Caller cancellation before this final check
                    // must leave both the store and runtime untouched.
                    authority.ensure_runtime_delivery_allowed(
                        runtime_id.as_deref(),
                        runtime_generation,
                    )?;

                    let screen = Arc::clone(&entry.screen);
                    let input = entry.input.clone();
                    let continued_service = Arc::clone(&service);
                    let continued_owner = owner.clone();
                    let continued_id = id.clone();
                    let continued_screen = Arc::clone(&screen);
                    let continued_input = input.clone();
                    let continued_runtime_id = runtime_id.clone();
                    let continued_interaction = interaction.clone();
                    let rejected_service = Arc::clone(&service);
                    let rejected_id = id.clone();
                    let rejected_screen = Arc::clone(&screen);
                    let rejected_interaction = interaction.clone();
                    append_with(
                        entry,
                        EventKind::TerminalResized {
                            columns: geometry.columns(),
                            rows: geometry.rows(),
                        },
                        |record| record.session.geometry = geometry,
                        move |committed| async move {
                            let result = match continued_interaction.resume_before_delivery().await
                            {
                                Ok(authority) => {
                                    continued_service
                                        .finish_terminal_resize(
                                            &continued_owner,
                                            &continued_id,
                                            TerminalResizeTarget {
                                                runtime_id: continued_runtime_id.as_deref(),
                                                runtime_generation,
                                                screen: &continued_screen,
                                                input: continued_input.as_ref(),
                                            },
                                            &committed,
                                            &authority,
                                        )
                                        .await
                                }
                                Err(error) => Err(error),
                            };
                            drop(input_fence);
                            (result, (lifecycle, continued_interaction))
                        },
                        move |_error| async move {
                            rejected_service
                                .synchronize_ordered_live_tail_for(&rejected_id, &rejected_screen)
                                .await;
                            drop(rejected_interaction);
                        },
                        drop_undelivered,
                    )
                };
                commit.committed().await.map_err(store_failed)?
            },
            |_loss, _result, _interaction| {},
        )
        .await
    }

    fn require_resize_entry(entry: &SessionEntry, owner: &SubjectId) -> Result<(), CoreError> {
        if !entry.session.is_owned_by(owner) {
            return Err(CoreError::Forbidden);
        }
        if entry.session.status.is_terminal() {
            return Err(CoreError::SessionTerminated);
        }
        if entry.session.status.is_rotating() {
            return Err(CoreError::SessionRotating);
        }
        entry.session.capabilities.require(Capability::Terminal)
    }

    fn require_same_resize_target(
        entry: &SessionEntry,
        owner: &SubjectId,
        runtime_id: Option<&str>,
        runtime_generation: u64,
        screen: &Arc<Mutex<Box<dyn ScreenModel>>>,
        input: Option<&TerminalInputSender>,
    ) -> Result<(), CoreError> {
        Self::require_resize_entry(entry, owner)?;
        if entry.runtime_generation != runtime_generation
            || entry.runtime.as_ref().map(|runtime| runtime.0.as_str()) != runtime_id
        {
            return Err(CoreError::RuntimeUnavailable(
                "the live terminal belongs to another runtime authority".into(),
            ));
        }
        if !Arc::ptr_eq(&entry.screen, screen) {
            return Err(CoreError::RuntimeUnavailable(
                "the live terminal changed while its resize was committing".into(),
            ));
        }
        let same_input = match (entry.input.as_ref(), input) {
            (Some(current), Some(expected)) => current.same_channel(expected),
            (None, None) => true,
            _ => false,
        };
        if !same_input {
            return Err(CoreError::RuntimeUnavailable(
                "the live terminal input changed while its resize was committing".into(),
            ));
        }
        Ok(())
    }

    async fn finish_terminal_resize(
        &self,
        owner: &SubjectId,
        id: &SessionId,
        target: TerminalResizeTarget<'_>,
        committed: &SessionRecord,
        authority: &RuntimeInteractionAuthority,
    ) -> Result<(), CoreError> {
        let TerminalResizeTarget {
            runtime_id,
            runtime_generation,
            screen,
            input,
        } = target;
        let geometry = committed.session.geometry;
        authority.ensure_runtime_delivery_allowed(runtime_id, runtime_generation)?;
        if let Some(sender) = input {
            // Keep runtime output from being folded at the old geometry after
            // the runtime has accepted this resize. The relay takes this same
            // lock before applying and broadcasting each output frame.
            let mut screen_guard = screen.lock().await;

            // Validate the captured sender under the service map, then release
            // that global lock before awaiting its receipt. The caller holds the
            // per-session input fence across this whole continuation, so a
            // rotation cannot replace the validated link before publication.
            {
                let entries = self.entries.read().await;
                let entry = entries.get(id).ok_or_else(|| {
                    CoreError::RuntimeUnavailable("this session has no live terminal entry".into())
                })?;
                Self::require_same_resize_target(
                    entry,
                    owner,
                    runtime_id,
                    runtime_generation,
                    screen,
                    input,
                )?;
            }
            authority.ensure_runtime_delivery_allowed(runtime_id, runtime_generation)?;
            self.forward_input(
                id,
                sender,
                TerminalInput::Resize(TermSize {
                    cols: geometry.columns(),
                    rows: geometry.rows(),
                }),
            )
            .await?;

            // A successful receipt makes publication mandatory. Revalidate
            // identity, but do not recheck the expiring authority afterwards.
            let mut entries = self.entries.write().await;
            let entry = entries.get_mut(id).ok_or_else(|| {
                CoreError::RuntimeUnavailable(
                    "the live terminal ended after accepting its resize".into(),
                )
            })?;
            Self::require_same_resize_target(
                entry,
                owner,
                runtime_id,
                runtime_generation,
                screen,
                input,
            )?;
            Self::install_committed_resize(entry, committed);
            screen_guard.resize(geometry);
        } else {
            // A live entry can intentionally have no runtime input link while
            // its screen remains useful. In that case the same final authority
            // check admits a local-only update of the durable desired state.
            {
                let mut entries = self.entries.write().await;
                let entry = entries.get_mut(id).ok_or_else(|| {
                    CoreError::RuntimeUnavailable("this session has no live terminal entry".into())
                })?;
                Self::require_same_resize_target(
                    entry,
                    owner,
                    runtime_id,
                    runtime_generation,
                    screen,
                    input,
                )?;
                authority.ensure_runtime_delivery_allowed(runtime_id, runtime_generation)?;
                Self::install_committed_resize(entry, committed);
            }
            screen.lock().await.resize(geometry);
        }
        Ok(())
    }

    fn install_committed_resize(entry: &mut SessionEntry, committed: &SessionRecord) {
        entry.session.geometry = committed.session.geometry;
        entry.session.last_seq = committed.session.last_seq;
        let event_log = Arc::clone(&entry.event_log);
        event_log.synchronize_tail(&mut entry.session.last_seq);
    }

    /// Whether a client is currently holding this session's terminal.
    ///
    /// Asked by an interface that is about to write on behalf of somebody who
    /// is *not* that client. Keystrokes from elsewhere land in the middle of
    /// whatever escape sequence the holder is drawing, and afterwards neither
    /// side can tell which characters were whose.
    ///
    /// A question rather than a refusal, because the answer is not the same for
    /// every caller: the holder drives the terminal through the same calls, and
    /// a guard buried in them would refuse the attachment its own input.
    pub async fn terminal_is_held(
        &self,
        caller: &Caller,
        id: &SessionId,
    ) -> Result<bool, CoreError> {
        self.authorize(caller, id).await?;
        let entries = self.entries.read().await;
        let Some(entry) = entries.get(id) else {
            drop(entries);
            let rotation_in_progress = self.rotation_in_progress(id).await;
            return Err(self
                .missing_live_session_error(caller, id, rotation_in_progress)
                .await);
        };
        Ok(entry.attached.is_some())
    }

    /// Read the current screen without holding a stream.
    ///
    /// Owner-only, on the same terms as reading the transcript: this is the
    /// same content by another route. A session's screen carries whatever the
    /// agent has printed, which includes the values of the environment bundle
    /// it was started with — so a capability that opens it is a capability
    /// that reads other people's secrets, and no operator holds one.
    pub async fn screen_snapshot(
        &self,
        caller: &Caller,
        id: &SessionId,
    ) -> Result<ScreenSnapshot, CoreError> {
        let screen = {
            let entries = self.entries.read().await;
            let entry = entries.get(id).ok_or(CoreError::SessionNotFound)?;
            if !entry.session.is_owned_by(&caller.subject) {
                return Err(CoreError::Forbidden);
            }
            Arc::clone(&entry.screen)
        };
        let snapshot = screen.lock().await.snapshot();
        Ok(snapshot)
    }
}

#[cfg(test)]
#[path = "terminal_io_tests.rs"]
mod tests;
