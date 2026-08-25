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
    AdapterError, ContentBlock, ConversationInput, ConversationInputSender, PermissionDecision,
};

use crate::authz::Caller;
use crate::capabilities::Capability;
use crate::error::CoreError;
use crate::event::EventKind;
use crate::ids::SessionId;
use crate::log::append;
use crate::service::SessionService;
use crate::session::SessionStatus;

/// What a session has used, as its runtime reports it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Usage {
    /// Tokens sent to the model.
    pub input_tokens: u64,
    /// Tokens the model produced.
    pub output_tokens: u64,
    /// How much of the model's context the conversation currently fills.
    pub context_used: u64,
    /// How large that context is, where the runtime states one.
    pub context_window: Option<u64>,
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
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
        text: String,
    ) -> Result<(), CoreError> {
        let _input_fence = self.input_gates.gate(id).read_owned().await;
        let input = self
            .conversation_input(caller, id, Capability::Conversation)
            .await?;
        // Recorded before the turn begins: the owner's message is what the
        // conversation replays as the owner's side of it, and its time is what
        // §18.1 labels that turn with.
        let owner_text = text.clone();
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
            append(
                entry,
                EventKind::OwnerMessage {
                    content: owner_text,
                },
            );
            append(entry, EventKind::TurnStarted);
            append(
                entry,
                EventKind::StatusChanged {
                    status: SessionStatus::Running,
                    stop_reason: None,
                    detail: None,
                    capability: None,
                },
            );
        }
        self.flush(id).await;
        Ok(())
    }

    /// Abandon the turn in flight, leaving the conversation open.
    ///
    /// Its own ability rather than part of holding a conversation. A kernel's
    /// wire may offer a prompt and no way to take it back, and a session that
    /// accepted the request and did nothing would leave whoever asked watching
    /// the agent carry on with no explanation.
    pub async fn interrupt_turn(
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
    ) -> Result<(), CoreError> {
        let _input_fence = self.input_gates.gate(id).read_owned().await;
        let input = self
            .conversation_input(caller, id, Capability::Interrupt)
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
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
        request_id: String,
        decision: ApprovalDecision,
    ) -> Result<(), CoreError> {
        let _input_fence = self.input_gates.gate(id).read_owned().await;
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
                        capability: None,
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
                context_window,
            } = &event.kind
            {
                total.input_tokens = total.input_tokens.max(*input_tokens);
                total.output_tokens = total.output_tokens.max(*output_tokens);
                // A turn's own accounting has no context reading and writes
                // zero. It is not a reading of an empty context, so it leaves
                // the latest real reading standing.
                if *context_used > 0 {
                    total.context_used = *context_used;
                }
                // Absence means this report did not state a ceiling, not that
                // a previously stated ceiling stopped applying. A later
                // stated value wins because a route can change mid-session.
                if let Some(window) = context_window {
                    total.context_window = Some(*window);
                }
            }
            total
        }))
    }

    /// The channel toward a session's runtime, if the caller may drive it.
    ///
    /// Takes the runtime back over where this process has never seen it. The
    /// sandbox outlives a control plane, so a restart loses the channel and not
    /// the conversation — and the terminal plane has done this at its own
    /// entry point since sessions became durable. Without it here, a
    /// conversation session comes back listed, intact, and refusing every
    /// prompt, which is a lie its owner cannot act on.
    async fn conversation_input(
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
        capability: Capability,
    ) -> Result<ConversationInputSender, CoreError> {
        // Read from the record rather than from what this process happens to
        // hold, because after a restart it holds nothing — and a session it has
        // never seen is not a session that does not exist.
        let record = self.record(id).await?;
        if !record.session.is_owned_by(&caller.subject) {
            return Err(CoreError::Forbidden);
        }
        if record.session.status.is_terminal() {
            return Err(CoreError::SessionTerminated);
        }
        // `is_interactive` names the wider UI state contract, but only a
        // rotation changes this operation's existing answer: provisioning and
        // idle sessions still fall through to their established runtime error.
        if record.session.status.is_rotating() {
            return Err(CoreError::SessionRotating);
        }
        let entries = self.entries.read().await;
        if !entries.contains_key(id) {
            let rotation_in_progress = self.rotation_in_progress(id).await;
            drop(entries);
            if rotation_in_progress {
                return Err(CoreError::SessionRotating);
            }
            self.readopt(&record).await?;
        } else {
            drop(entries);
        }
        let entries = self.entries.read().await;
        let Some(entry) = entries.get(id) else {
            let rotation_in_progress = self.rotation_in_progress(id).await;
            drop(entries);
            return Err(self
                .missing_live_session_error(caller, id, rotation_in_progress)
                .await);
        };
        // Recheck the live entry after readoption and after the durable read.
        // A handover may have begun while either awaited operation was in
        // flight, and its outgoing channel remains present until teardown.
        if entry.session.status.is_rotating() {
            return Err(CoreError::SessionRotating);
        }
        entry.session.capabilities.require(capability)?;
        entry.conversation.clone().ok_or_else(|| {
            CoreError::RuntimeUnavailable("this session holds no conversation".into())
        })
    }

    /// Send something to a runtime, saying so if it did not arrive.
    async fn toward_runtime(
        &self,
        id: &SessionId,
        input: &ConversationInputSender,
        message: ConversationInput,
    ) -> Result<(), CoreError> {
        input.send(message).await.map_err(|error| {
            tracing::warn!(session_id = %id, %error, "conversation input was not delivered");
            match error {
                AdapterError::Unavailable(detail) => CoreError::RuntimeUnavailable(detail),
                AdapterError::Rejected(detail)
                | AdapterError::AtCapacity(detail)
                | AdapterError::Unsupported(detail) => CoreError::RuntimeRefused(detail),
            }
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

#[cfg(test)]
#[path = "conversation_operation_tests.rs"]
mod conversation_operation_tests;
