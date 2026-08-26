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

use std::collections::HashSet;
use std::{sync::Arc, time::Duration};

use layer3_adapter::{
    AdapterError, ContentBlock, ConversationInput, ConversationInputSender, PermissionDecision,
};

use crate::authz::{Caller, Capability as AuthorizationCapability};
use crate::capabilities::Capability;
use crate::error::CoreError;
use crate::event::EventKind;
use crate::ids::SessionId;
use crate::log::append_batch;
use crate::service::{store_failed, SessionService};
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

/// Why waiting for a session's conversation did not reach a usable state.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConversationReadinessError {
    /// The session could not become conversable for a reason already named by
    /// the core, such as ownership, termination, or a missing capability.
    #[error(transparent)]
    Refused(#[from] CoreError),
    /// Provisioning did not publish a usable conversation before the caller's
    /// deadline.
    #[error("session provisioning timed out before its conversation became ready")]
    ProvisioningTimedOut,
}

const CONVERSATION_READINESS_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// One answer in an atomic approval-resolution request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalResolution {
    /// The runtime request being answered.
    pub request_id: String,
    /// What the owner decided.
    pub decision: ApprovalDecision,
}

impl ApprovalResolution {
    /// Name one request and the decision to deliver for it.
    pub fn new(request_id: impl Into<String>, decision: ApprovalDecision) -> Self {
        Self {
            request_id: request_id.into(),
            decision,
        }
    }
}

impl SessionService {
    /// Wait for a provisioning session to accept conversation input.
    ///
    /// Readiness means more than a durable `Ready` status: the runtime's live
    /// entry and conversation sender must also have been published. The local
    /// bring-up claim spans that gap, so waiting on it avoids opening a second
    /// conversation beside one that is still being installed. A ready session
    /// with no local entry is readopted, preserving operation after a control
    /// plane restart. The supplied bound covers both publication and that
    /// readoption wait.
    pub async fn await_conversable(
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
        bound: Duration,
    ) -> Result<(), ConversationReadinessError> {
        // Refuse before observing the session or its provisioning timing.
        caller
            .authorization
            .require(AuthorizationCapability::UseOwnSessions)?;

        let wait_for_conversation = async {
            loop {
                let record = self.record(id).await?;
                if !record.session.is_owned_by(&caller.subject) {
                    return Err(CoreError::Forbidden);
                }
                if record.session.status.is_terminal() {
                    return Err(CoreError::SessionTerminated);
                }
                if !record.session.capabilities.unrecorded() {
                    record
                        .session
                        .capabilities
                        .require(Capability::Conversation)?;
                }

                match record.session.status {
                    SessionStatus::Provisioning => {}
                    SessionStatus::Ready | SessionStatus::Running => {
                        let conversation_is_live = self
                            .entries
                            .read()
                            .await
                            .get(id)
                            .and_then(|entry| entry.conversation.as_ref())
                            .is_some();
                        if conversation_is_live {
                            self.conversation_input(caller, id, Capability::Conversation)
                                .await?;
                            return Ok(());
                        }
                        if self.starting.read().await.contains(id) {
                            tokio::time::sleep(CONVERSATION_READINESS_POLL_INTERVAL).await;
                            continue;
                        }
                        self.conversation_input(caller, id, Capability::Conversation)
                            .await?;
                        return Ok(());
                    }
                    SessionStatus::Rotating => return Err(CoreError::SessionRotating),
                    SessionStatus::Idle => return Err(CoreError::InvalidState),
                    SessionStatus::Terminated => unreachable!("terminal status handled above"),
                }

                tokio::time::sleep(CONVERSATION_READINESS_POLL_INTERVAL).await;
            }
        };

        tokio::time::timeout(bound, wait_for_conversation)
            .await
            .map_err(|_| ConversationReadinessError::ProvisioningTimedOut)??;
        Ok(())
    }

    /// Open a prompt turn on a session's conversation.
    ///
    /// The turn is recorded as beginning here rather than waited for, because
    /// this is where it begins: a runtime has nothing to announce until the
    /// model answers, and a client that prompted and saw nothing recorded
    /// cannot tell a slow agent from a lost prompt.
    ///
    /// Returns the exact durable sequence committed for this prompt's
    /// [`EventKind::TurnStarted`] boundary.
    pub async fn send_prompt(
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
        text: String,
    ) -> Result<u64, CoreError> {
        // One prompt must publish its Running transition before the next one
        // decides whether it is steering. This gate is deliberately separate
        // from the lifecycle gate `conversation_input` may acquire in readoption.
        let _admission = self.prompt_gates.gate(id).lock_owned().await;
        // A recorded running conversation can be refused before recovery is
        // attempted, so unsupported mid-turn input reaches neither a runtime
        // nor the log. An older record may still carry an unrecorded grant;
        // `conversation_input` settles that record, and the live check below
        // applies the same rule after it has done so.
        let record = self.record(id).await?;
        if !record.session.is_owned_by(&caller.subject) {
            return Err(CoreError::Forbidden);
        }
        if record.session.status.is_terminal() {
            return Err(CoreError::SessionTerminated);
        }
        if record.session.status == SessionStatus::Running
            && !record.session.capabilities.unrecorded()
        {
            record
                .session
                .capabilities
                .require(Capability::Conversation)?;
            record.session.capabilities.require(Capability::Steer)?;
        }

        // Readopt before taking the update-ordering gate. Opening a conversation
        // can itself report sign-in and command updates, and those must remain
        // foldable while lifecycle recovery runs.
        self.conversation_input(caller, id, Capability::Conversation)
            .await?;
        let input_fence = self.input_gates.gate(id).read_owned().await;
        let ordering = self.conversation_order_gates.gate(id);
        let ordered_turn = ordering.lock().await;
        // The live entry may have changed during readoption. Recheck both prompt
        // capabilities and drive the sender that is current under the same gate
        // its folder takes before recording delivery effects.
        let input = {
            let entries = self.entries.read().await;
            let entry = entries.get(id).ok_or(CoreError::SessionNotFound)?;
            if !entry.session.is_owned_by(&caller.subject) {
                return Err(CoreError::Forbidden);
            }
            if entry.session.status.is_terminal() {
                return Err(CoreError::SessionTerminated);
            }
            if entry.session.status.is_rotating() {
                return Err(CoreError::SessionRotating);
            }
            entry
                .session
                .capabilities
                .require(Capability::Conversation)?;
            if entry.session.status == SessionStatus::Running {
                entry.session.capabilities.require(Capability::Steer)?;
            }
            entry.conversation.clone().ok_or_else(|| {
                CoreError::RuntimeUnavailable("this session holds no conversation".into())
            })?
        };
        // The boundary comes first so every projection can establish the turn
        // before it receives the message that opened it. The owner's message
        // still carries its own durable time, which is what §18.1 labels that
        // side of the conversation with.
        let owner_text = text.clone();
        self.toward_runtime(
            id,
            &input,
            ConversationInput::Prompt(vec![ContentBlock::from(text)]),
        )
        .await?;
        let committed = {
            let mut entries = self.entries.write().await;
            let Some(entry) = entries.get_mut(id) else {
                return Err(CoreError::SessionNotFound);
            };
            entry.session.status = SessionStatus::Running;
            entry.session.stop_reason = None;
            append_batch(
                entry,
                [
                    EventKind::TurnStarted,
                    EventKind::OwnerMessage {
                        content: owner_text,
                    },
                    EventKind::StatusChanged {
                        status: SessionStatus::Running,
                        stop_reason: None,
                        detail: None,
                        capability: None,
                    },
                ],
            )
        };
        // The lane owns the complete prompt unit in its required order. Let the
        // folder enqueue immediate runtime effects behind it while persistence
        // settles; the completion below still cannot pass those earlier events.
        drop(ordered_turn);
        drop(input_fence);
        let record = committed.committed().await.map_err(store_failed)?;
        record.session.last_seq.checked_sub(2).ok_or_else(|| {
            CoreError::RuntimeUnavailable(
                "session store: committed prompt boundary had no valid sequence".into(),
            )
        })
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
        self.conversation_input(caller, id, Capability::Interrupt)
            .await?;
        let input_fence = self.input_gates.gate(id).read_owned().await;
        let input = {
            let entries = self.entries.read().await;
            let entry = entries.get(id).ok_or(CoreError::SessionNotFound)?;
            if !entry.session.is_owned_by(&caller.subject) {
                return Err(CoreError::Forbidden);
            }
            if entry.session.status.is_terminal() {
                return Err(CoreError::SessionTerminated);
            }
            if entry.session.status.is_rotating() {
                return Err(CoreError::SessionRotating);
            }
            entry.session.capabilities.require(Capability::Interrupt)?;
            entry.conversation.clone().ok_or_else(|| {
                CoreError::RuntimeUnavailable("this session holds no conversation".into())
            })?
        };
        let delivered = self
            .toward_runtime(id, &input, ConversationInput::Cancel)
            .await;
        drop(input_fence);
        delivered
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
        self.resolve_approvals(caller, id, &[ApprovalResolution::new(request_id, decision)])
            .await
    }

    /// Answer one atomic batch of outstanding runtime approval requests.
    ///
    /// Every identifier is validated before the first answer is sent. If the
    /// adapter accepts a prefix and then refuses a later item, that accepted
    /// prefix is recorded durably before the adapter error is returned.
    pub async fn resolve_approvals(
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
        resolutions: &[ApprovalResolution],
    ) -> Result<(), CoreError> {
        if resolutions.is_empty() {
            return Err(CoreError::InvalidState);
        }

        // Readoption may itself open a runtime update stream. Complete it before
        // taking the folder's order gate, then re-read every live authority
        // under that gate rather than using the sender snapshot returned here.
        self.conversation_input(caller, id, Capability::Conversation)
            .await?;
        let input_fence = self.input_gates.gate(id).read_owned().await;
        let ordering = self.conversation_order_gates.gate(id);
        let ordered_resolution = ordering.lock().await;
        let input = {
            let entries = self.entries.read().await;
            let entry = entries.get(id).ok_or(CoreError::SessionNotFound)?;
            if !entry.session.is_owned_by(&caller.subject) {
                return Err(CoreError::Forbidden);
            }
            if entry.session.status.is_terminal() {
                return Err(CoreError::SessionTerminated);
            }
            if entry.session.status.is_rotating() {
                return Err(CoreError::SessionRotating);
            }
            entry
                .session
                .capabilities
                .require(Capability::Conversation)?;

            let mut unique = HashSet::with_capacity(resolutions.len());
            if resolutions.iter().any(|resolution| {
                !unique.insert(resolution.request_id.as_str())
                    || !entry.pending_approvals.contains(&resolution.request_id)
            }) {
                // Refused before delivery. A runtime handed an answer to
                // something it is not waiting for may ignore it or act on the
                // wrong request, and a partially validated batch cannot retry.
                return Err(CoreError::InvalidState);
            }
            entry.conversation.clone().ok_or_else(|| {
                CoreError::RuntimeUnavailable("this session holds no conversation".into())
            })?
        };

        let mut delivered: usize = 0;
        let mut delivery_error = None;
        for resolution in resolutions {
            match self
                .toward_runtime(
                    id,
                    &input,
                    ConversationInput::PermissionAnswer {
                        id: resolution.request_id.clone(),
                        decision: intent_for(resolution.decision),
                    },
                )
                .await
            {
                Ok(()) => delivered += 1,
                Err(error) => {
                    delivery_error = Some(error);
                    break;
                }
            }
        }

        let commit = if delivered == 0 {
            None
        } else {
            let mut entries = self.entries.write().await;
            let Some(entry) = entries.get_mut(id) else {
                return Err(CoreError::SessionNotFound);
            };
            let mut events = Vec::with_capacity(delivered.saturating_add(1));
            for resolution in &resolutions[..delivered] {
                let removed = entry.pending_approvals.remove(&resolution.request_id);
                debug_assert!(removed, "a prevalidated approval remains outstanding");
                // Answered, so the session is no longer waiting on a person. It
                // goes back to running rather than ready: the interrupted turn
                // is still in flight. Preserve the singular operation's order
                // by placing this transition immediately before the final
                // resolution that empties the outstanding set.
                if entry.pending_approvals.is_empty() && entry.session.status == SessionStatus::Idle
                {
                    entry.session.status = SessionStatus::Running;
                    entry.session.stop_reason = None;
                    events.push(EventKind::StatusChanged {
                        status: SessionStatus::Running,
                        stop_reason: None,
                        detail: None,
                        capability: None,
                    });
                }
                events.push(EventKind::ApprovalResolved {
                    request_id: resolution.request_id.clone(),
                    allowed: matches!(resolution.decision, ApprovalDecision::Allow),
                });
            }
            Some(append_batch(entry, events))
        };

        // Immediate TurnEnded updates emitted by a permission sender may now be
        // folded, but their commits enter the same lane behind the accepted
        // prefix above. Storage waiting must never retain the folder's gate.
        drop(ordered_resolution);
        drop(input_fence);
        if let Some(commit) = commit {
            commit.committed().await.map_err(store_failed)?;
        }
        if let Some(error) = delivery_error {
            return Err(error);
        }
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
        caller
            .authorization
            .require(AuthorizationCapability::UseOwnSessions)?;
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
        let missing_live_entry = !self.entries.read().await.contains_key(id);
        if missing_live_entry {
            let rotation_in_progress = self.rotation_in_progress(id).await;
            if rotation_in_progress {
                return Err(CoreError::SessionRotating);
            }
            self.readopt(&record).await?;
        }
        let entries = self.entries.read().await;
        let Some(entry) = entries.get(id) else {
            drop(entries);
            let rotation_in_progress = self.rotation_in_progress(id).await;
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
