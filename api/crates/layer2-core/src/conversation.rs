//! The conversation plane: what a session says, and what it is asked.
//!
//! Split from the service so that file stays about session state and this one
//! is about what an agent is doing. Everything here folds one direction —
//! updates from a runtime become durable events — or drives the other, which
//! is prompts, answers and cancellation going back down.
//!
//! ## Durable before emitted
//!
//! An update is recorded and then shown, never the other way round. A client
//! that saw something the log does not hold would find it missing on reconnect
//! and have no way to tell that from having imagined it, which is the failure
//! the cursor-based replay exists to make impossible.
//!
//! ## What is durable, and what is only alive
//!
//! A committed message is history and is recorded. The token-level pieces it
//! was assembled from are liveness: they say what is happening rather than what
//! happened, and giving them sequence numbers would punch permanent holes in
//! the series a resuming client counts on — or, worse, replay somebody's
//! conversation being typed a second time.
//!
//! ## Naming
//!
//! The vocabulary here is the core's own. A runtime's words for a tool's kind
//! or a call's status are carried through opaquely and never branched on: the
//! sets differ between runtimes and grow without warning, and a session must
//! not fail because a kernel invented a category.

use std::sync::Arc;

use layer3_adapter::{
    ContentBlock, ConversationInput, ConversationUpdate, PermissionDecision, PermissionRequest,
    PromptResponse, SessionUpdate,
};
use tokio::sync::mpsc;

use crate::authz::Caller;
use crate::capabilities::Capability;
use crate::error::CoreError;
use crate::event::{EventKind, LiveFrame};
use crate::ids::SessionId;
use crate::service::{append, SessionService};
use crate::session::{SessionStatus, StopReason};

/// What a session has used, as its runtime reports it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Usage {
    /// Tokens sent to the model.
    pub input_tokens: u64,
    /// Tokens the model produced.
    pub output_tokens: u64,
    /// How much of the model's context the conversation currently fills.
    pub context_used: u64,
}

/// How an owner answered something the runtime was blocked on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// Let it proceed.
    Allow,
    /// Refuse it. The turn carries on without doing this.
    Deny,
    /// Refuse it and stop the turn.
    ///
    /// A distinct answer rather than a stronger "no": refusing one command and
    /// abandoning what the agent was doing are different intentions, and a
    /// person who meant the second and got the first watches the agent try
    /// something else instead of stopping.
    Abort,
}

impl SessionService {
    /// Open a prompt turn on a session's conversation.
    ///
    /// The turn is recorded as beginning here rather than waited for, because
    /// this is where it begins: a runtime has nothing to announce until the
    /// model answers, and a client that prompted and saw nothing recorded
    /// cannot tell a slow agent from a lost prompt.
    pub async fn send_prompt(
        &self,
        caller: &Caller,
        id: &SessionId,
        text: String,
    ) -> Result<(), CoreError> {
        let input = self
            .conversation_input(caller, id, Capability::Conversation)
            .await?;
        self.toward_runtime(
            id,
            &input,
            ConversationInput::Prompt(vec![ContentBlock::from(text)]),
        )
        .await?;
        {
            let mut entries = self.entries.write().await;
            let Some(entry) = entries.get_mut(id) else {
                return Err(CoreError::SessionNotFound);
            };
            entry.session.status = SessionStatus::Running;
            entry.session.stop_reason = None;
            append(entry, EventKind::TurnStarted);
            append(
                entry,
                EventKind::StatusChanged {
                    status: SessionStatus::Running,
                    stop_reason: None,
                    detail: None,
                },
            );
        }
        self.flush(id).await;
        Ok(())
    }

    /// Abandon the turn in flight, leaving the conversation open.
    pub async fn interrupt_turn(&self, caller: &Caller, id: &SessionId) -> Result<(), CoreError> {
        let input = self
            .conversation_input(caller, id, Capability::Conversation)
            .await?;
        self.toward_runtime(id, &input, ConversationInput::Cancel)
            .await
    }

    /// Answer something the runtime is blocked on.
    ///
    /// The finer shape of the answer travels down to the runtime, which is the
    /// only thing that can act on it. What is recorded is whether the owner
    /// allowed it, because that is the question every interface projects and
    /// the one anybody reading the log later is asking.
    pub async fn resolve_approval(
        &self,
        caller: &Caller,
        id: &SessionId,
        request_id: String,
        decision: ApprovalDecision,
    ) -> Result<(), CoreError> {
        let input = self
            .conversation_input(caller, id, Capability::Conversation)
            .await?;
        let outstanding = {
            let entries = self.entries.read().await;
            entries
                .get(id)
                .is_some_and(|e| e.pending_approvals.contains(&request_id))
        };
        if !outstanding {
            // Refused rather than forwarded. A runtime handed an answer to
            // something it is not waiting for will either ignore it or act on
            // the wrong thing, and neither is visible to whoever answered.
            return Err(CoreError::InvalidState);
        }

        self.toward_runtime(
            id,
            &input,
            ConversationInput::PermissionAnswer {
                id: request_id.clone(),
                decision: intent_for(decision),
            },
        )
        .await?;

        {
            let mut entries = self.entries.write().await;
            let Some(entry) = entries.get_mut(id) else {
                return Err(CoreError::SessionNotFound);
            };
            entry.pending_approvals.remove(&request_id);
            // Answered, so the session is no longer waiting on a person. It
            // goes back to running rather than to ready: the turn that stopped
            // to ask is still in flight.
            if entry.pending_approvals.is_empty() && entry.session.status == SessionStatus::Idle {
                entry.session.status = SessionStatus::Running;
                entry.session.stop_reason = None;
                append(
                    entry,
                    EventKind::StatusChanged {
                        status: SessionStatus::Running,
                        stop_reason: None,
                        detail: None,
                    },
                );
            }
            append(
                entry,
                EventKind::ApprovalResolved {
                    request_id,
                    allowed: matches!(decision, ApprovalDecision::Allow),
                },
            );
        }
        self.flush(id).await;
        Ok(())
    }

    /// What this session has used.
    ///
    /// The highest totals its runtimes have reported, rather than the latest.
    /// A session carried onto a replacement runtime is counted afresh by that
    /// runtime, and taking the latest would show somebody their usage falling;
    /// adding them together would count the history the new runtime resumed
    /// from twice. Neither is right, and the highest is the one that cannot
    /// understate what has been spent.
    pub async fn get_usage(&self, caller: &Caller, id: &SessionId) -> Result<Usage, CoreError> {
        let record = self.record(id).await?;
        if !record.session.is_owned_by(&caller.subject) {
            return Err(CoreError::Forbidden);
        }
        record.session.capabilities.require(Capability::Usage)?;
        let events = self.events_since(caller, id, None).await?;
        Ok(events.iter().fold(Usage::default(), |mut total, event| {
            if let EventKind::UsageReported {
                input_tokens,
                output_tokens,
                context_used,
            } = &event.kind
            {
                total.input_tokens = total.input_tokens.max(*input_tokens);
                total.output_tokens = total.output_tokens.max(*output_tokens);
                // Not a maximum: this is how full the context is right now,
                // and a conversation that was compacted genuinely holds less
                // than it did.
                total.context_used = *context_used;
            }
            total
        }))
    }

    /// The channel toward a session's runtime, if the caller may drive it.
    async fn conversation_input(
        &self,
        caller: &Caller,
        id: &SessionId,
        capability: Capability,
    ) -> Result<mpsc::Sender<ConversationInput>, CoreError> {
        self.authorize(caller, id).await?;
        let entries = self.entries.read().await;
        let entry = entries.get(id).ok_or(CoreError::SessionNotFound)?;
        entry.session.capabilities.require(capability)?;
        entry.conversation.clone().ok_or_else(|| {
            CoreError::RuntimeUnavailable("this session holds no conversation".into())
        })
    }

    /// Send something to a runtime, saying so if it did not arrive.
    async fn toward_runtime(
        &self,
        id: &SessionId,
        input: &mpsc::Sender<ConversationInput>,
        message: ConversationInput,
    ) -> Result<(), CoreError> {
        input.send(message).await.map_err(|_| {
            tracing::warn!(session_id = %id, "the conversation channel closed");
            CoreError::RuntimeUnavailable("this session's conversation has ended".into())
        })
    }
}

/// What the runtime is told an owner decided.
///
/// The two vocabularies are deliberately separate — the core says what a
/// person chose, the contract says what a runtime is given — so this is the
/// single place they meet, and the only place a new answer has to be taught to
/// cross.
fn intent_for(decision: ApprovalDecision) -> PermissionDecision {
    match decision {
        ApprovalDecision::Allow => PermissionDecision::Allow,
        ApprovalDecision::Deny => PermissionDecision::Deny,
        ApprovalDecision::Abort => PermissionDecision::Abort,
    }
}

impl SessionService {
    /// Record one thing a runtime said.
    ///
    /// Recorded and then shown, never the other way round: a client that saw
    /// something the log does not hold would find it missing on reconnect with
    /// no way to tell that from having imagined it.
    async fn fold_update(&self, id: &SessionId, update: ConversationUpdate) {
        let update = match update {
            ConversationUpdate::Said(update) => update,
            ConversationUpdate::Asked(request) => {
                self.fold_request(id, request).await;
                return;
            }
            ConversationUpdate::TurnEnded(completed) => {
                self.fold_completion(id, completed).await;
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
                self.flush(id).await;
                return;
            }
        };
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
            self.flush(id).await;
            return;
        }

        let Some(kind) = durable(update) else {
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
            append(entry, kind);
        }
        self.flush(id).await;
    }

    /// Record something the runtime is blocked on, and park the session.
    async fn fold_request(&self, id: &SessionId, request: PermissionRequest) {
        {
            let mut entries = self.entries.write().await;
            let Some(entry) = entries.get_mut(id) else {
                return;
            };
            entry.pending_approvals.insert(request.id.clone());
            // Waiting on a person is not a failure and not the end of a turn.
            // `RequiresAction` is the one stop reason that is explicitly not
            // terminal, which is what lets a client show this as a question
            // rather than as a session that died.
            entry.session.status = SessionStatus::Idle;
            entry.session.stop_reason = Some(StopReason::RequiresAction);
            append(
                entry,
                EventKind::ApprovalRequested {
                    request_id: request.id.clone(),
                    action: request.request.tool_call.fields.title.clone(),
                    detail: None,
                },
            );
            append(
                entry,
                EventKind::StatusChanged {
                    status: SessionStatus::Idle,
                    stop_reason: Some(StopReason::RequiresAction),
                    detail: None,
                },
            );
        }
        self.flush(id).await;
    }

    /// Record that a turn ended, and why.
    ///
    /// The reason is the difference between an agent that finished, one that
    /// was interrupted and one that refused, and a client showing all three
    /// the same way is showing none of them.
    async fn fold_completion(&self, id: &SessionId, completed: PromptResponse) {
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
                    },
                );
            }
            // The turn ending is what finishes the message the agent was
            // saying. Committed before the boundary so the log reads in the
            // order it happened.
            if let Some((_, content)) = entry.assembling.take() {
                append(entry, EventKind::MessageCommitted { content });
            }
            entry.session.status = SessionStatus::Ready;
            entry.session.stop_reason = Some(reason);
            append(entry, EventKind::TurnCompleted);
            append(
                entry,
                EventKind::StatusChanged {
                    status: SessionStatus::Ready,
                    stop_reason: Some(reason),
                    detail: None,
                },
            );
        }
        self.flush(id).await;
    }

    /// Close a turn the runtime left open when its conversation ended.
    ///
    /// A dangling turn sits in the log forever looking like work in progress,
    /// which is indistinguishable from an agent that is simply slow. The
    /// reason it actually stopped for is recorded instead.
    async fn close_open_turn(&self, id: &SessionId) {
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
            if entry.session.status == SessionStatus::Running {
                entry.session.status = SessionStatus::Idle;
                entry.session.stop_reason = Some(StopReason::Error);
                append(entry, EventKind::TurnCompleted);
                append(
                    entry,
                    EventKind::StatusChanged {
                        status: SessionStatus::Idle,
                        stop_reason: Some(StopReason::Error),
                        detail: Some("the runtime's conversation ended mid-turn".to_string()),
                    },
                );
            }
        }
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
fn durable(update: SessionUpdate) -> Option<EventKind> {
    match update {
        SessionUpdate::AgentMessageChunk(_) => None,
        SessionUpdate::ToolCall(call) => Some(EventKind::ToolCallStarted {
            call_id: call.tool_call_id.0.to_string(),
            title: call.title,
            kind: format!("{:?}", call.kind).to_lowercase(),
        }),
        SessionUpdate::ToolCallUpdate(update) => Some(EventKind::ToolCallUpdated {
            call_id: update.tool_call_id.0.to_string(),
            status: update
                .fields
                .status
                .map(|s| format!("{s:?}").to_lowercase())
                // A runtime that says a call moved without saying where it got
                // to has still said something worth recording: the call is not
                // where it was.
                .unwrap_or_else(|| "updated".to_string()),
        }),
        SessionUpdate::UsageUpdate(usage) => Some(EventKind::UsageReported {
            input_tokens: 0,
            output_tokens: 0,
            context_used: usage.used,
        }),
        // Everything else the protocol can say is either liveness this core
        // does not record, or a runtime's own configuration surface, which is
        // not something a session's history is about.
        _ => None,
    }
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
    mut updates: mpsc::Receiver<ConversationUpdate>,
) {
    tokio::spawn(async move {
        while let Some(update) = updates.recv().await {
            service.fold_update(&id, update).await;
        }
        // The channel ending is the runtime's conversation ending. A turn left
        // open would sit in the log forever looking like work still in
        // progress, so it is closed with the reason it actually stopped for.
        service.close_open_turn(&id).await;
        tracing::info!(session_id = %id, "conversation folding ended");
    });
}

#[cfg(test)]
#[path = "conversation_tests.rs"]
mod conversation_tests;

#[cfg(test)]
#[path = "conversation_operation_tests.rs"]
mod conversation_operation_tests;
