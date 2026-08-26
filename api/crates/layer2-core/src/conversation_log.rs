//! Folding a runtime's conversation into a session's durable log.
//!
//! Split from the operations so both stay within the project's file-size
//! limit, on a real division: that file is what an owner does to a session,
//! and this is what its runtime says happened.
//!
//! ## Durable before emitted
//!
//! An update is recorded and then shown, never the other way round. A client
//! that saw something the log does not hold would find it missing on reconnect
//! and have no way to tell that from having imagined it, which is the failure
//! the cursor-based replay exists to make impossible.
//!
//! ## Naming
//!
//! The vocabulary written here is the core's own. A runtime's words for a
//! tool's kind or a call's status are carried through opaquely and never
//! branched on: the sets differ between runtimes and grow without warning, and
//! a session must not fail because a kernel invented a category.

use std::{collections::HashMap, sync::Arc};

use layer3_adapter::{
    ContentBlock, ConversationUpdate, PermissionLocator, PermissionRequest, PromptResponse,
    SessionUpdate,
};
use tokio::sync::mpsc;

use crate::event::{ConfigOption, Event, EventKind, LiveFrame, PlanStep, ToolBody};
use crate::ids::SessionId;
use crate::log::append;
use crate::service::SessionService;
use crate::session::{SessionStatus, StopReason};

/// Rebuild the approval requests a live entry must still accept.
///
/// Requests and resolutions are both durable, so replaying them in sequence is
/// the authority after a process restart. No separate stored set can drift
/// from this fold.
#[derive(Debug)]
pub(crate) struct ApprovalState {
    pub(crate) pending: HashMap<String, Option<PermissionLocator>>,
    pub(crate) locators: HashMap<String, Option<PermissionLocator>>,
}

pub(crate) fn pending_approvals_from(events: &[Event]) -> Result<ApprovalState, String> {
    let mut pending = HashMap::new();
    let mut locators = HashMap::new();
    for event in events {
        match &event.kind {
            EventKind::ApprovalRequested {
                request_id,
                locator,
                ..
            } => {
                if let Some(previous) = locators.get(request_id) {
                    if previous != locator {
                        return Err(
                            "an approval id was reused with a different delivery locator".into(),
                        );
                    }
                    continue;
                }
                locators.insert(request_id.clone(), locator.clone());
                pending.insert(request_id.clone(), locator.clone());
            }
            EventKind::ApprovalResolved { request_id, .. } => {
                pending.remove(request_id);
            }
            EventKind::TurnStarted
            | EventKind::TurnCompleted
            | EventKind::ConversationOpened { .. }
            | EventKind::PlaneChanged { .. }
            | EventKind::RuntimeLost { .. } => {
                pending.clear();
                locators.clear();
            }
            EventKind::StatusChanged {
                status: SessionStatus::Idle,
                stop_reason: Some(StopReason::RequiresAction),
                ..
            } => {}
            EventKind::StatusChanged {
                status: SessionStatus::Running,
                ..
            } => pending.clear(),
            EventKind::StatusChanged { .. } => {
                pending.clear();
                locators.clear();
            }
            _ => {}
        }
    }
    Ok(ApprovalState { pending, locators })
}

impl SessionService {
    /// Record one thing a runtime said.
    ///
    /// Recorded and then shown, never the other way round: a client that saw
    /// something the log does not hold would find it missing on reconnect with
    /// no way to tell that from having imagined it.
    async fn fold_update(
        &self,
        id: &SessionId,
        update: ConversationUpdate,
        ordering: &tokio::sync::Mutex<()>,
    ) {
        // Rotation takes these gates in the same order before publishing its
        // durable boundary. Keeping that order here makes every admitted
        // runtime update finish its mutation before rotation can begin.
        let input_fence = self.input_gates.gate(id).read_owned().await;
        let ordered = ordering.lock().await;
        let update = match update {
            ConversationUpdate::Said(update) => update,
            ConversationUpdate::Asked(request) => {
                self.fold_request(id, request, input_fence, ordered).await;
                return;
            }
            ConversationUpdate::TurnEnded(completed) => {
                self.fold_completion(id, completed, input_fence, ordered)
                    .await;
                return;
            }
            ConversationUpdate::SignInRequired { url, code, message } => {
                self.fold_sign_in(id, url, code, message, input_fence, ordered)
                    .await;
                return;
            }
            ConversationUpdate::Failed {
                message,
                capability,
            } => {
                self.fold_failure(id, message, capability, input_fence, ordered)
                    .await;
                return;
            }
            ConversationUpdate::Lost { frames } => {
                // Recorded where it happened, in the same spirit as a
                // rotation's timestamp: the owner is shown a boundary rather
                // than left to puzzle over a conversation that skips.
                tracing::warn!(session_id = %id, frames, "part of this conversation was lost");
                {
                    let mut entries = self.entries.write().await;
                    let Some(entry) = entries.get_mut(id) else {
                        return;
                    };
                    append(entry, EventKind::ConversationGap { frames });
                }
                drop(ordered);
                drop(input_fence);
                self.flush(id).await;
                return;
            }
        };
        // A piece of the agent's reasoning. Liveness and nothing else: unlike a
        // message it is never committed, so it accumulates nowhere and the
        // durable log does not learn it happened. Handled before the message
        // arm below rather than inside it, because the one thing they must not
        // share is the accumulation.
        if let SessionUpdate::AgentThoughtChunk(chunk) = &update {
            let Some(text) = plain_text(&chunk.content) else {
                return;
            };
            let live = {
                let entries = self.entries.read().await;
                let Some(entry) = entries.get(id) else {
                    return;
                };
                entry.live_frames.clone()
            };
            // No flush: nothing was appended and nothing changed about the
            // session. A thought that reaches nobody is simply unwatched.
            let _ = live.send(LiveFrame::ThoughtDelta(text));
            return;
        }
        // A piece of a message is liveness, and is shown rather than recorded.
        // What is recorded is the message the pieces add up to, at the moment
        // the runtime moves on from it.
        if let SessionUpdate::AgentMessageChunk(chunk) = &update {
            let Some(text) = plain_text(&chunk.content) else {
                return;
            };
            let message_id = chunk.message_id.as_ref().map(|id| id.0.to_string());
            let mut finished = None;
            let live = {
                let mut entries = self.entries.write().await;
                let Some(entry) = entries.get_mut(id) else {
                    return;
                };
                match &mut entry.assembling {
                    // The same message, still being typed.
                    Some((assembling, so_far)) if *assembling == message_id => {
                        so_far.push_str(&text)
                    }
                    // A different one. The previous is finished by definition:
                    // a runtime that has started saying something else is not
                    // going back to what it was saying.
                    Some(_) => {
                        finished = entry.assembling.take();
                        entry.assembling = Some((message_id, text.clone()));
                    }
                    None => entry.assembling = Some((message_id, text.clone())),
                }
                if let Some((_, content)) = finished {
                    append(entry, EventKind::MessageCommitted { content });
                }
                entry.live_frames.clone()
            };
            // A send with no subscriber is not an error: the session is simply
            // unwatched, and the whole message is committed either way.
            let _ = live.send(LiveFrame::MessageDelta(text));
            drop(ordered);
            drop(input_fence);
            self.flush(id).await;
            return;
        }

        /*
         * The actions the owner may invoke, sent live *and* recorded.
         *
         * This was liveness only, on the reasoning that a stale list replayed
         * from the log would offer actions that no longer exist. The reasoning
         * was sound and the outcome was not: the kernel announces this once,
         * as the conversation opens, which is before any client has subscribed
         * — so the send had no receiver and the list reached nobody, every
         * session, every time. The protocol has no request to ask again, so
         * there was no recovering it either.
         *
         * A list one announcement old beats the empty list that was the
         * alternative, and it is replaced whole each time the kernel speaks.
         */
        if let SessionUpdate::AvailableCommandsUpdate(commands) = &update {
            let commands: Vec<crate::event::AvailableCommand> = commands
                .available_commands
                .iter()
                .map(|command| crate::event::AvailableCommand {
                    name: command.name.clone(),
                    description: command.description.clone(),
                })
                .collect();
            let live = {
                let mut entries = self.entries.write().await;
                match entries.get_mut(id) {
                    Some(entry) => {
                        append(
                            entry,
                            EventKind::AvailableCommandsSet {
                                commands: commands.clone(),
                            },
                        );
                        Some(entry.live_frames.clone())
                    }
                    // Announced before the entry exists, which for this update
                    // is the ordinary case rather than a fault.
                    None => None,
                }
            };
            match live {
                Some(live) => {
                    let _ = live.send(LiveFrame::AvailableCommands(commands));
                    drop(ordered);
                    drop(input_fence);
                    self.flush(id).await;
                }
                None => {
                    drop(ordered);
                    drop(input_fence);
                    self.append_without_entry(id, EventKind::AvailableCommandsSet { commands })
                        .await;
                }
            }
            return;
        }

        // Read the grant and the values to redact before folding. A tool body
        // is only recorded when the owner asked for it, and it never carries a
        // value the session was handed — the credential, or an environment
        // variable's value.
        let (record_tool_bodies, secrets) = {
            let entries = self.entries.read().await;
            let Some(entry) = entries.get(id) else {
                return;
            };
            let mut secrets = entry.environment_values.clone();
            if let Some(credential) = entry.subject_credential.as_ref() {
                secrets.push(credential.clone());
            }
            (entry.session.options.record_tool_bodies, secrets)
        };
        let reading = match &update {
            SessionUpdate::UsageUpdate(usage) if usage.used > 0 => {
                Some((usage.used, (usage.size > 0).then_some(usage.size)))
            }
            _ => None,
        };
        let Some(kind) = durable(update, record_tool_bodies, &secrets) else {
            // A kind of update this core has no word for. Skipped rather than
            // guessed at: the vocabulary here is the core's own, and inventing
            // an event for something it does not model would put a runtime's
            // shape into the one layer that must outlive it.
            return;
        };
        {
            let mut entries = self.entries.write().await;
            let Some(entry) = entries.get_mut(id) else {
                return;
            };
            if let Some((now, window)) = reading {
                if let Some(was) = entry.context_reading.filter(|was| *was > now) {
                    append(entry, EventKind::ContextCompacted { was, now, window });
                }
                entry.context_reading = Some(now);
            }
            append(entry, kind);
        }
        drop(ordered);
        drop(input_fence);
        self.flush(id).await;
    }

    /// Record something the runtime is blocked on, and park the session.
    async fn fold_request(
        &self,
        id: &SessionId,
        request: PermissionRequest,
        input_fence: tokio::sync::OwnedRwLockReadGuard<()>,
        ordered: tokio::sync::MutexGuard<'_, ()>,
    ) {
        let locator = request.locator.clone();
        {
            let mut entries = self.entries.write().await;
            let Some(entry) = entries.get_mut(id) else {
                drop(entries);
                self.fold_early_request(id, request, input_fence, ordered)
                    .await;
                return;
            };
            if let Some(previous) = entry.approval_locators.get(&request.id) {
                if previous != &locator {
                    tracing::error!(
                        session_id = %id,
                        "a permission request id was reused with a different locator"
                    );
                }
                return;
            }
            entry
                .pending_approvals
                .insert(request.id.clone(), locator.clone());
            entry
                .approval_locators
                .insert(request.id.clone(), locator.clone());
            // Waiting on a person is not a failure and not the end of a turn.
            // `RequiresAction` is the one stop reason that is explicitly not
            // terminal, which is what lets a client show this as a question
            // rather than as a session that died.
            let rotating = entry.session.status.is_rotating();
            if !rotating {
                entry.session.status = SessionStatus::Idle;
                entry.session.stop_reason = Some(StopReason::RequiresAction);
            }
            append(
                entry,
                EventKind::ApprovalRequested {
                    request_id: request.id.clone(),
                    action: request.request.tool_call.fields.title.clone(),
                    detail: None,
                    locator,
                },
            );
            if !rotating {
                append(
                    entry,
                    EventKind::StatusChanged {
                        status: SessionStatus::Idle,
                        stop_reason: Some(StopReason::RequiresAction),
                        detail: None,
                        capability: None,
                    },
                );
            }
        }
        drop(ordered);
        drop(input_fence);
        self.flush(id).await;
    }

    /// Record a request replayed while readoption has not installed its entry.
    async fn fold_early_request(
        &self,
        id: &SessionId,
        request: PermissionRequest,
        input_fence: tokio::sync::OwnedRwLockReadGuard<()>,
        ordered: tokio::sync::MutexGuard<'_, ()>,
    ) {
        let mut record = match self.store.get(id).await {
            Ok(Some(record)) => record,
            Ok(None) => {
                tracing::error!(session_id = %id, "cannot record an early permission request: the session is absent");
                return;
            }
            Err(error) => {
                tracing::error!(session_id = %id, %error, "cannot record an early permission request");
                return;
            }
        };
        let history = match self.readoption_history(&record).await {
            Ok(history) => history,
            Err(error) => {
                tracing::error!(
                    session_id = %id,
                    %error,
                    "cannot verify an early permission request"
                );
                return;
            }
        };
        if let Some(previous) = history.approval_locators.get(&request.id) {
            if previous != &request.locator {
                tracing::error!(
                    session_id = %id,
                    "a replayed permission request id changed its locator"
                );
            }
            return;
        }

        let rotating = record.session.status.is_rotating();
        if !rotating {
            record.session.status = SessionStatus::Idle;
            record.session.stop_reason = Some(StopReason::RequiresAction);
        }
        let mut events = vec![Event {
            session_id: id.clone(),
            seq: 0,
            kind: EventKind::ApprovalRequested {
                request_id: request.id,
                action: request.request.tool_call.fields.title,
                detail: None,
                locator: request.locator,
            },
            appended_at: String::new(),
        }];
        if !rotating {
            events.push(Event {
                session_id: id.clone(),
                seq: 0,
                kind: EventKind::StatusChanged {
                    status: SessionStatus::Idle,
                    stop_reason: Some(StopReason::RequiresAction),
                    detail: None,
                    capability: None,
                },
                appended_at: String::new(),
            });
        }
        if let Err(error) = self.persist_durably(&mut record, events).await {
            tracing::error!(
                session_id = %id,
                %error,
                "cannot record an early permission request"
            );
        }
        // Installation takes the same gates before its final history read. The
        // guards are released only after this direct append has settled.
        drop(ordered);
        drop(input_fence);
    }

    /// Record that a turn ended, and why.
    ///
    /// The reason is the difference between an agent that finished, one that
    /// was interrupted and one that refused, and a client showing all three
    /// the same way is showing none of them.
    async fn fold_completion(
        &self,
        id: &SessionId,
        completed: PromptResponse,
        input_fence: tokio::sync::OwnedRwLockReadGuard<()>,
        ordered: tokio::sync::MutexGuard<'_, ()>,
    ) {
        {
            let mut entries = self.entries.write().await;
            let Some(entry) = entries.get_mut(id) else {
                return;
            };
            let reason = stop_reason_for(&completed);
            // Reported here because this is where a runtime reports it: the
            // turn's response carries the counts, and the update stream
            // carries only how full the context is.
            if let Some(used) = completed.usage.as_ref() {
                append(
                    entry,
                    EventKind::UsageReported {
                        input_tokens: used.input_tokens,
                        output_tokens: used.output_tokens,
                        // Not stated on a turn's own accounting, and left as it
                        // was rather than guessed at: the update stream is what
                        // says how full the context is.
                        context_used: 0,
                        // A turn's accounting has no context-window field, so
                        // there is no ceiling here to state or infer.
                        context_window: None,
                    },
                );
            }
            // The turn ending is what finishes the message the agent was
            // saying. Committed before the boundary so the log reads in the
            // order it happened.
            if let Some((_, content)) = entry.assembling.take() {
                append(entry, EventKind::MessageCommitted { content });
            }
            entry.pending_approvals.clear();
            entry.approval_locators.clear();
            let rotating = entry.session.status.is_rotating();
            if !rotating {
                entry.session.status = SessionStatus::Ready;
                entry.session.stop_reason = Some(reason);
            }
            append(entry, EventKind::TurnCompleted);
            if !rotating {
                append(
                    entry,
                    EventKind::StatusChanged {
                        status: SessionStatus::Ready,
                        stop_reason: Some(reason),
                        detail: None,
                        capability: None,
                    },
                );
            }
        }
        drop(ordered);
        drop(input_fence);
        self.flush(id).await;
    }

    /// Record that the agent is waiting for its owner to sign in.
    ///
    /// Durable rather than live-only. This is the one thing on this plane a
    /// person has to act on somewhere else, and a prompt that vanished with a
    /// reload would leave a session waiting on a code nobody can see any more.
    ///
    /// What is written is an address and a short-lived code, and nothing else
    /// could be: the credential the sign-in produces is written by the agent
    /// inside its own sandbox and never reaches this service.
    async fn fold_sign_in(
        &self,
        id: &SessionId,
        url: String,
        code: Option<String>,
        message: Option<String>,
        input_fence: tokio::sync::OwnedRwLockReadGuard<()>,
        ordered: tokio::sync::MutexGuard<'_, ()>,
    ) {
        tracing::info!(session_id = %id, "this session is waiting for its owner to sign in");
        {
            let mut entries = self.entries.write().await;
            if let Some(entry) = entries.get_mut(id) {
                append(entry, EventKind::SignInRequired { url, code, message });
                drop(entries);
                drop(ordered);
                drop(input_fence);
                self.flush(id).await;
                return;
            }
        }
        /*
         * No entry yet, which for this one update is the normal case rather
         * than the exception. `entries` holds sessions that have a running
         * runtime, and a kernel raises this from inside `open_conversation`,
         * before that is true — then waits there for as long as a person takes
         * to act on it. Returning here is what left an own-account session
         * sitting in `provisioning` with nothing in its log to say why.
         *
         * So it goes to the store directly, the way provisioning progress
         * already does. The sequence comes from the same shared allocator, so
         * the two paths cannot hand out the same number.
         */
        drop(ordered);
        drop(input_fence);
        self.append_without_entry(id, EventKind::SignInRequired { url, code, message })
            .await;
    }

    /// Append one event for a session that has no live entry yet.
    ///
    /// Deliberately not offered to `fold_update` at large: most updates
    /// describe a conversation that is already running, and one arriving with
    /// no entry means something is wrong rather than something is early. Two
    /// legitimately precede the runtime being ready — a sign-in the owner must
    /// complete before anything can start, and the kernel's announcement of
    /// what it offers, which it makes as the conversation opens. Both say so
    /// at their call sites.
    async fn append_without_entry(&self, id: &SessionId, kind: EventKind) {
        let Ok(Some(mut record)) = self.store.get(id).await else {
            tracing::error!(
                session_id = %id,
                event = kind.tag(),
                "cannot record this event: the session is not in the store"
            );
            return;
        };
        if let Err(e) = self
            .persist_durably(
                &mut record,
                vec![Event {
                    session_id: id.clone(),
                    seq: 0,
                    kind,
                    appended_at: String::new(),
                }],
            )
            .await
        {
            /*
             * Error, not a warning, and never a silent return. A dropped
             * sign-in leaves a session waiting on a prompt nobody will ever
             * see, and the whole of #351 was that this path failed quietly
             * underneath a `tracing::info!` that read like success.
             */
            tracing::error!(
                session_id = %id,
                error = %e,
                "cannot record this event"
            );
        }
    }

    /// Record a turn that stopped because something went wrong.
    ///
    /// Modelled on `close_open_turn` rather than on `fold_completion`: what
    /// happened is a failure, not an ending, and the difference is exactly
    /// what lets a client tell "the agent had nothing more to say" from "the
    /// agent could not answer". The reason travels as the status change's
    /// detail, which every interface already projects — so every caller is
    /// told the same thing without any interface being taught a new shape.
    async fn fold_failure(
        &self,
        id: &SessionId,
        message: String,
        capability: Option<String>,
        input_fence: tokio::sync::OwnedRwLockReadGuard<()>,
        ordered: tokio::sync::MutexGuard<'_, ()>,
    ) {
        tracing::warn!(session_id = %id, %message, "a turn failed");
        {
            let mut entries = self.entries.write().await;
            let Some(entry) = entries.get_mut(id) else {
                return;
            };
            // Half a message is still what the agent said, and it is the last
            // thing anybody saw. Committed before the failure so the log
            // reads in the order it happened.
            if let Some((_, content)) = entry.assembling.take() {
                append(entry, EventKind::MessageCommitted { content });
            }
            entry.pending_approvals.clear();
            entry.approval_locators.clear();
            let rotating = entry.session.status.is_rotating();
            if !rotating {
                entry.session.status = SessionStatus::Ready;
                entry.session.stop_reason = Some(StopReason::Error);
            }
            append(entry, EventKind::TurnCompleted);
            if !rotating {
                append(
                    entry,
                    EventKind::StatusChanged {
                        status: SessionStatus::Ready,
                        stop_reason: Some(StopReason::Error),
                        detail: Some(message),
                        capability,
                    },
                );
            }
        }
        drop(ordered);
        drop(input_fence);
        self.flush(id).await;
    }

    /// Close a turn the runtime left open when its conversation ended.
    ///
    /// A dangling turn sits in the log forever looking like work in progress,
    /// which is indistinguishable from an agent that is simply slow. The
    /// reason it actually stopped for is recorded instead.
    async fn close_open_turn(&self, id: &SessionId, ordering: &tokio::sync::Mutex<()>) {
        let input_fence = self.input_gates.gate(id).read_owned().await;
        let ordered = ordering.lock().await;
        {
            let mut entries = self.entries.write().await;
            let Some(entry) = entries.get_mut(id) else {
                return;
            };
            if let Some((_, content)) = entry.assembling.take() {
                // Half a message is still what the agent said. Losing it
                // because the runtime went away would leave the log missing
                // the last thing anybody saw on screen.
                append(entry, EventKind::MessageCommitted { content });
            }
            // Only a turn that was actually open is closed. A session that
            // was idle when its runtime went away has nothing dangling, and
            // writing a failure against it would report one that did not
            // happen.
            if entry.session.status == SessionStatus::Running || !entry.pending_approvals.is_empty()
            {
                entry.pending_approvals.clear();
                entry.approval_locators.clear();
                let rotating = entry.session.status.is_rotating();
                if !rotating {
                    entry.session.status = SessionStatus::Idle;
                    entry.session.stop_reason = Some(StopReason::Error);
                }
                append(entry, EventKind::TurnCompleted);
                if !rotating {
                    append(
                        entry,
                        EventKind::StatusChanged {
                            status: SessionStatus::Idle,
                            stop_reason: Some(StopReason::Error),
                            detail: Some("the runtime's conversation ended mid-turn".to_string()),
                            capability: None,
                        },
                    );
                }
            }
        }
        drop(ordered);
        drop(input_fence);
        self.flush(id).await;
    }
}

/// The core's word for why a turn ended.
///
/// The two vocabularies are separate on purpose — one describes a turn and the
/// other describes a session — so this is the single place they meet.
fn stop_reason_for(completed: &PromptResponse) -> StopReason {
    use layer3_adapter::TurnStopReason;
    match completed.stop_reason {
        TurnStopReason::EndTurn => StopReason::EndTurn,
        // A turn the client cancelled is not a session that stopped: the
        // conversation is open and the next prompt continues it.
        TurnStopReason::Cancelled => StopReason::EndTurn,
        TurnStopReason::MaxTokens | TurnStopReason::MaxTurnRequests => StopReason::BudgetReached,
        TurnStopReason::Refusal => StopReason::RequiresAction,
        // A reason this build has no word for. Read as an ordinary ending
        // rather than as a failure: a newer protocol naming a new way for a
        // turn to finish must not make a healthy session look broken.
        _ => StopReason::EndTurn,
    }
}

/// The core's word for what a runtime said, if it has one.
///
/// A runtime's own words for a tool's kind or a call's status are carried
/// through opaquely rather than mapped: those sets differ between runtimes and
/// grow without warning, and a session must not fail because a kernel invented
/// a category.
fn durable(
    update: SessionUpdate,
    record_tool_bodies: bool,
    secrets: &[String],
) -> Option<EventKind> {
    match update {
        SessionUpdate::AgentMessageChunk(_) => None,
        SessionUpdate::ToolCall(call) => Some(EventKind::ToolCallStarted {
            call_id: call.tool_call_id.0.to_string(),
            title: call.title,
            kind: wire_name(&call.kind),
            arguments: if record_tool_bodies {
                body_of(call.raw_input, secrets)
            } else {
                None
            },
        }),
        SessionUpdate::ToolCallUpdate(update) => Some(EventKind::ToolCallUpdated {
            call_id: update.tool_call_id.0.to_string(),
            status: update
                .fields
                .status
                .map(|status| wire_name(&status))
                // A runtime that says a call moved without saying where it got
                // to has still said something worth recording: the call is not
                // where it was.
                .unwrap_or_else(|| "updated".to_string()),
            result: if record_tool_bodies {
                body_of(update.fields.raw_output, secrets)
            } else {
                None
            },
        }),
        SessionUpdate::UsageUpdate(usage) => Some(EventKind::UsageReported {
            input_tokens: 0,
            output_tokens: 0,
            context_used: usage.used,
            // The harness begins at zero and fills this when its route states
            // a window. Until then zero means "not stated", not no context.
            context_window: (usage.size > 0).then_some(usage.size),
        }),
        // A permission mode change is a safety matter: it decides how much the
        // agent may do unattended, so it is recorded rather than only shown.
        SessionUpdate::CurrentModeUpdate(update) => Some(EventKind::ModeChanged {
            mode: update.current_mode_id.0.to_string(),
        }),
        // The agent's plan, a full revision each time. Steps are carried in the
        // kernel's own words, never mapped.
        SessionUpdate::Plan(plan) => Some(EventKind::PlanSet {
            entries: plan
                .entries
                .into_iter()
                .map(|entry| PlanStep {
                    title: entry.content,
                    status: wire_name(&entry.status),
                })
                .collect(),
        }),
        // The settings the kernel says are settable, so a client can render a
        // selection rather than a free-text box.
        SessionUpdate::ConfigOptionUpdate(update) => Some(EventKind::ConfigOptionsSet {
            options: update
                .config_options
                .into_iter()
                .map(|option| ConfigOption {
                    id: option.id.0.to_string(),
                    name: option.name,
                    kind: wire_name(&option.kind),
                })
                .collect(),
        }),
        // Everything else the protocol can say is either liveness this core
        // does not record, or a runtime's own configuration surface, which is
        // not something a session's history is about.
        _ => None,
    }
}

/// A runtime's own name for something, as it spells it on the wire.
///
/// Taken from serialization rather than from the Rust `Debug` output. These
/// strings land in durable events and are read back for the life of a session,
/// so they have to be the ones the protocol defines: a `Debug` spelling is an
/// implementation detail of a struct in a dependency, and renaming one there
/// would quietly change what this service recorded about work already done.
///
/// A value that will not serialize to a plain string is recorded as unknown
/// rather than as a rendering of its structure. Nothing branches on these, so
/// an unfamiliar one costs a label and not a session.
fn wire_name<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => name,
        _ => "unknown".to_string(),
    }
}

/// Longest tool body recorded.
///
/// The same bound as an environment value (`MAX_VALUE`), so a tool's output
/// cannot become a file store through the conversation log.
const TOOL_BODY_CAP: usize = 8192;

/// A tool call's arguments or result, redacted and bounded.
///
/// `None` for a runtime that offered none, or a body that redacts to nothing.
/// The replacement is of exact known values only — what the core handed the
/// session — and is deliberately not a pattern matcher over arbitrary text:
/// a matcher would miss things and be sold as complete, where this is honest
/// about redacting only what it knows.
fn body_of(raw: Option<serde_json::Value>, secrets: &[String]) -> Option<ToolBody> {
    let raw = raw?;
    let mut text = raw.to_string();
    for secret in secrets {
        if !secret.is_empty() {
            text = text.replace(secret.as_str(), "[redacted]");
        }
    }
    let mut truncated = false;
    if text.len() > TOOL_BODY_CAP {
        truncated = true;
        let mut cut = TOOL_BODY_CAP;
        while cut > 0 && !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
    }
    Some(ToolBody { text, truncated })
}

/// The text of a content block, if it is text at all.
///
/// Images and embedded resources are not deltas of a message being typed, and
/// rendering one into a string here would put a placeholder in somebody's
/// conversation where the thing itself belongs.
fn plain_text(content: &ContentBlock) -> Option<String> {
    match content {
        ContentBlock::Text(text) => Some(text.text.clone()),
        _ => None,
    }
}

/// Fold a runtime's conversation into a session's durable log.
///
/// Runs for the session's life, independent of any client, exactly as the
/// terminal relay does. A session nobody is watching still records what its
/// agent did, which is what makes the log a record rather than a broadcast.
pub(crate) fn spawn_folder(
    service: Arc<SessionService>,
    id: SessionId,
    runtime_generation: u64,
    mut updates: mpsc::Receiver<ConversationUpdate>,
) {
    // Kept alive for the folder's lifetime so every prompt and every update
    // for this runtime meet at the same per-session gate. Each fold drops its
    // guard after mutation and enqueue, before waiting for store durability:
    // the event lane preserves that order without letting slow I/O block the
    // next operation's admission. The gate is deliberately independent of
    // lifecycle installation because this folder starts first while a
    // conversation can already be reporting useful updates.
    let ordering = service.conversation_order_gates.gate(&id);
    tokio::spawn(async move {
        while let Some(update) = updates.recv().await {
            service.fold_update(&id, update, &ordering).await;
        }
        // The channel ending is the runtime's conversation ending. A turn left
        // open would sit in the log forever looking like work still in
        // progress, so it is closed with the reason it actually stopped for.
        service.close_open_turn(&id, &ordering).await;
        tracing::info!(session_id = %id, runtime_generation, "conversation folding ended");
    });
}

#[cfg(test)]
#[path = "conversation_tests.rs"]
mod conversation_tests;
