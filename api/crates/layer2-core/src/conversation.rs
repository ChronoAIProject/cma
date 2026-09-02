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

use std::collections::{BTreeMap, HashSet};
use std::{sync::Arc, time::Duration};

use layer3_adapter::{
    AdapterError, ContentBlock, ConversationInput, ConversationInputSender, PermissionDecision,
};

use crate::authz::{Caller, Capability as AuthorizationCapability};
use crate::capabilities::Capability;
use crate::conversation_opening::ConversationLinkId;
use crate::coordination::ConversationLink;
use crate::error::CoreError;
use crate::event::EventKind;
use crate::ids::SessionId;
use crate::log::{append_batch, append_conversation_with, drop_undelivered};
use crate::service::{store_failed, SessionService};
use crate::session::SessionStatus;
use crate::store::{SessionRecord, StoreError};

/// Maximum number of caller metadata pairs retained beside one prompt turn.
pub const MAX_TURN_METADATA_PAIRS: usize = 16;
/// Maximum Unicode scalar count accepted for one turn metadata key.
pub const MAX_TURN_METADATA_KEY_CHARS: usize = 64;
/// Maximum Unicode scalar count accepted for one turn metadata value.
pub const MAX_TURN_METADATA_VALUE_CHARS: usize = 512;

/// Fixed race-time refusal returned when a caller's linear conversation view
/// is no longer current.
pub const CONVERSATION_ADVANCED_DETAIL: &str =
    "the conversation advanced beyond the expected previous turn";
/// Fixed refusal returned when a linear conversation already has a live turn.
pub const CONVERSATION_IN_PROGRESS_DETAIL: &str =
    "the conversation already has a response in progress";

/// Durable, protocol-neutral context for an interface that admits only a new
/// turn at the current end of a conversation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct LinearPromptContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) expected_previous_turn: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) metadata: BTreeMap<String, String>,
}

impl LinearPromptContext {
    fn new(
        expected_previous_turn: Option<u64>,
        metadata: BTreeMap<String, String>,
    ) -> Result<Self, CoreError> {
        validate_turn_metadata(&metadata)?;
        Ok(Self {
            expected_previous_turn,
            metadata,
        })
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        validate_turn_metadata(&self.metadata).map_err(|error| error.to_string())
    }
}

fn validate_turn_metadata(metadata: &BTreeMap<String, String>) -> Result<(), CoreError> {
    if metadata.len() > MAX_TURN_METADATA_PAIRS {
        return Err(CoreError::InvalidRequest(format!(
            "turn metadata accepts at most {MAX_TURN_METADATA_PAIRS} pairs"
        )));
    }
    if metadata
        .keys()
        .any(|key| key.chars().count() > MAX_TURN_METADATA_KEY_CHARS)
    {
        return Err(CoreError::InvalidRequest(format!(
            "turn metadata keys accept at most {MAX_TURN_METADATA_KEY_CHARS} characters"
        )));
    }
    if metadata
        .values()
        .any(|value| value.chars().count() > MAX_TURN_METADATA_VALUE_CHARS)
    {
        return Err(CoreError::InvalidRequest(format!(
            "turn metadata values accept at most {MAX_TURN_METADATA_VALUE_CHARS} characters"
        )));
    }
    Ok(())
}

fn conversation_advanced() -> CoreError {
    CoreError::InvalidState {
        detail: Some(CONVERSATION_ADVANCED_DETAIL.into()),
    }
}

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

struct ReceivedHistoryTarget {
    runtime_generation: u64,
    conversation_link: Option<ConversationLinkId>,
    conversation_authority: Option<ConversationLink>,
    runtime_id: Option<String>,
    running_from: Option<SessionStatus>,
}

/// Fold cumulative runtime counters across durable authority incarnations.
///
/// Before exact authority markers existed, `Rotating` is the only retained
/// candidate boundary in the durable log. Legacy histories use it as their best
/// available approximation; it is not proof that authority changed at that
/// exact position. A marker-aware store instead records
/// `RuntimeHandoverStarted` before its first non-authoritative `Rotating`. That
/// durable discriminator permanently enables marker accounting for the
/// session, so no later status change closes a segment. Every compatibility
/// `RuntimeIncarnationStarted` closes the outgoing segment when its handle
/// becomes durable. A strictly newer coordinated `RuntimeStarted` closes the
/// outgoing segment at the exact authority switch; duplicate or older
/// generations are replay noise.
///
/// Context readings describe the conversation, not a runtime counter, so an
/// incarnation boundary resets only token maxima.
fn fold_usage<'a>(kinds: impl IntoIterator<Item = &'a EventKind>) -> Usage {
    let mut total = Usage::default();
    let mut segment_input_tokens = 0_u64;
    let mut segment_output_tokens = 0_u64;
    let mut exact_markers = false;
    let mut newest_generation = None;

    for kind in kinds {
        match kind {
            EventKind::RuntimeHandoverStarted => exact_markers = true,
            EventKind::RuntimeIncarnationStarted => {
                exact_markers = true;
                total.input_tokens = total.input_tokens.saturating_add(segment_input_tokens);
                total.output_tokens = total.output_tokens.saturating_add(segment_output_tokens);
                segment_input_tokens = 0;
                segment_output_tokens = 0;
            }
            EventKind::RuntimeStarted { generation } => {
                exact_markers = true;
                if newest_generation.is_none_or(|current| *generation > current) {
                    total.input_tokens = total.input_tokens.saturating_add(segment_input_tokens);
                    total.output_tokens = total.output_tokens.saturating_add(segment_output_tokens);
                    segment_input_tokens = 0;
                    segment_output_tokens = 0;
                    newest_generation = Some(*generation);
                }
            }
            EventKind::StatusChanged { status, .. } if !exact_markers && status.is_rotating() => {
                total.input_tokens = total.input_tokens.saturating_add(segment_input_tokens);
                total.output_tokens = total.output_tokens.saturating_add(segment_output_tokens);
                segment_input_tokens = 0;
                segment_output_tokens = 0;
            }
            EventKind::UsageReported {
                input_tokens,
                output_tokens,
                context_used,
                context_window,
            } => {
                segment_input_tokens = segment_input_tokens.max(*input_tokens);
                segment_output_tokens = segment_output_tokens.max(*output_tokens);
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
            _ => {}
        }
    }

    total.input_tokens = total.input_tokens.saturating_add(segment_input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(segment_output_tokens);
    total
}

impl SessionService {
    /// Publish the durable result of receipt history, then propose its optional
    /// running transition against that exact record and runtime authority.
    async fn finish_received_history(
        self: &Arc<Self>,
        id: &SessionId,
        target: ReceivedHistoryTarget,
        committed: SessionRecord,
    ) {
        let ReceivedHistoryTarget {
            runtime_generation,
            conversation_link,
            conversation_authority,
            runtime_id,
            running_from,
        } = target;
        let live_matches = self
            .install_received_history(
                id,
                runtime_generation,
                conversation_link,
                conversation_authority.as_ref(),
                runtime_id.as_deref(),
                &committed,
            )
            .await;
        let Some(running_from) = running_from else {
            return;
        };
        if !live_matches || committed.session.status != running_from {
            return;
        }

        // A history-only write may legitimately rebase over a handover. The
        // lifecycle proposal may not: it describes work accepted by one exact
        // conversation link and runtime authority.
        if conversation_authority.as_ref().is_some_and(|authority| {
            authority.runtime_generation != runtime_generation
                || Some(authority.runtime_id.as_str()) != runtime_id.as_deref()
        }) || committed.runtime_id != runtime_id
        {
            return;
        }

        let continued_service = Arc::clone(self);
        let continued_id = id.clone();
        let continued_runtime_id = runtime_id.clone();
        let continued_authority = conversation_authority.clone();
        let persisted_authority = conversation_authority.clone();
        let rejected_service = Arc::clone(self);
        let rejected_id = id.clone();
        let rejected_authority = conversation_authority.clone();
        let commit = {
            let mut entries = self.entries.write().await;
            let Some(entry) = entries.get_mut(id).filter(|entry| {
                entry.runtime_generation == runtime_generation
                    && entry.conversation_link == conversation_link
                    && conversation_authority.as_ref().is_none_or(|authority| {
                        entry.conversation_authority.as_ref() == Some(authority)
                    })
                    && entry.runtime.as_ref().map(|runtime| runtime.0.as_str())
                        == runtime_id.as_deref()
                    && entry.session.status == running_from
            }) else {
                return;
            };
            append_conversation_with(
                entry,
                EventKind::StatusChanged {
                    status: SessionStatus::Running,
                    stop_reason: None,
                    detail: None,
                    capability: None,
                },
                |record| {
                    record.session.status = SessionStatus::Running;
                    record.session.stop_reason = None;
                },
                persisted_authority,
                move |committed| async move {
                    let mut entries = continued_service.entries.write().await;
                    if let Some(entry) = entries.get_mut(&continued_id).filter(|entry| {
                        entry.runtime_generation == runtime_generation
                            && entry.conversation_link == conversation_link
                            && continued_authority.as_ref().is_none_or(|authority| {
                                entry.conversation_authority.as_ref() == Some(authority)
                            })
                            && entry.runtime.as_ref().map(|runtime| runtime.0.as_str())
                                == continued_runtime_id.as_deref()
                    }) {
                        if entry.session.status == running_from {
                            entry.session.status = committed.session.status;
                            entry.session.stop_reason = committed.session.stop_reason;
                        }
                        entry.session.last_seq = committed.session.last_seq;
                        let event_log = Arc::clone(&entry.event_log);
                        event_log.synchronize_tail(&mut entry.session.last_seq);
                    }
                    ((), ())
                },
                move |error| async move {
                    if matches!(error, StoreError::ConversationLinkChanged { .. }) {
                        if let (Some(link), Some(authority)) =
                            (conversation_link, rejected_authority.as_ref())
                        {
                            rejected_service
                                .retire_conversation_link(&rejected_id, link, authority)
                                .await;
                        }
                    }
                },
                drop_undelivered,
            )
        };

        if let Err(error) = commit.committed().await {
            tracing::debug!(
                session_id = %id,
                %error,
                "an optional running transition lost its exact durable boundary"
            );
            if let Ok(current) = self.record(id).await {
                self.install_received_history(
                    id,
                    runtime_generation,
                    conversation_link,
                    conversation_authority.as_ref(),
                    runtime_id.as_deref(),
                    &current,
                )
                .await;
            }
        }
    }

    /// Bring one still-current live entry to the lifecycle snapshot returned by
    /// its mandatory history commit without replacing runtime-local state.
    async fn install_received_history(
        &self,
        id: &SessionId,
        runtime_generation: u64,
        conversation_link: Option<ConversationLinkId>,
        conversation_authority: Option<&ConversationLink>,
        runtime_id: Option<&str>,
        committed: &SessionRecord,
    ) -> bool {
        let mut entries = self.entries.write().await;
        let Some(entry) = entries.get_mut(id).filter(|entry| {
            entry.runtime_generation == runtime_generation
                && entry.conversation_link == conversation_link
                && conversation_authority.is_none_or(|authority| {
                    entry.conversation_authority.as_ref() == Some(authority)
                })
                && entry.runtime.as_ref().map(|runtime| runtime.0.as_str()) == runtime_id
        }) else {
            return false;
        };
        if committed.runtime_id.as_deref() == runtime_id {
            entry.session.status = committed.session.status;
            entry.session.recovery_state = committed.session.recovery_state;
            entry.session.stop_reason = committed.session.stop_reason;
        }
        entry.session.last_seq = committed.session.last_seq;
        let event_log = Arc::clone(&entry.event_log);
        event_log.synchronize_tail(&mut entry.session.last_seq);
        true
    }

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
        let wait_for_conversation = async {
            loop {
                let session = self.session_for_interaction(caller, id).await?;
                if !session.capabilities.unrecorded() {
                    session.capabilities.require(Capability::Conversation)?;
                }

                match session.status {
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
                    SessionStatus::Rotating => {
                        unreachable!("interaction preflight refuses rotation")
                    }
                    SessionStatus::Idle => return Err(CoreError::invalid_state()),
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
        self.send_prompt_with_context(caller, id, text, None).await
    }

    /// Open a new prompt turn only at the caller's expected conversation end.
    ///
    /// Unlike [`Self::send_prompt`], this path never steers an in-flight turn.
    /// The expectation is revalidated while holding distributed interaction
    /// authority immediately before runtime delivery, so two replicas cannot
    /// both extend the same response branch.
    pub async fn send_linear_prompt(
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
        text: String,
        expected_previous_turn: Option<u64>,
        metadata: BTreeMap<String, String>,
    ) -> Result<u64, CoreError> {
        let context = LinearPromptContext::new(expected_previous_turn, metadata)?;
        self.send_prompt_with_context(caller, id, text, Some(context))
            .await
    }

    async fn send_prompt_with_context(
        self: &Arc<Self>,
        caller: &Caller,
        id: &SessionId,
        text: String,
        linear: Option<LinearPromptContext>,
    ) -> Result<u64, CoreError> {
        caller
            .authorization
            .require(AuthorizationCapability::UseOwnSessions)?;
        // A recorded running conversation can be refused before recovery is
        // attempted, so unsupported mid-turn input reaches neither a runtime
        // nor the log. An older record may still carry an unrecorded grant;
        // `conversation_input` settles that record, and the live check below
        // applies the same rule after it has done so.
        let record = self.record(id).await?;
        if !record.session.is_owned_by(&caller.subject) {
            return Err(CoreError::Forbidden);
        }
        if let Some(origin) = record.chat_origin.as_ref() {
            let hosts = self.chat_hosts.as_ref().ok_or({
                CoreError::ChatUnavailable(crate::ChatUnavailableCause::HostStoreUnavailable)
            })?;
            let host = hosts
                .get(&origin.host_id)
                .await
                .map_err(|_| {
                    CoreError::ChatUnavailable(crate::ChatUnavailableCause::HostStoreUnavailable)
                })?
                .ok_or(CoreError::ChatUnavailable(
                    crate::ChatUnavailableCause::Retired,
                ))?;
            if host.owner != caller.subject
                || !host.threads.iter().any(|slot| {
                    slot.session_id == *id
                        && slot.worktree_name == origin.worktree_name
                        && slot.worktree_path == origin.worktree_path
                        && !slot.removed
                })
            {
                return Err(CoreError::Forbidden);
            }
            match host.status {
                crate::ChatHostStatus::Pausing
                    if host.revive_cause == Some(crate::ChatReviveCause::ImageDrift) =>
                {
                    return Err(CoreError::SessionRotating);
                }
                crate::ChatHostStatus::Pausing => {
                    return Err(CoreError::ChatUnavailable(
                        crate::ChatUnavailableCause::RevivePending,
                    ));
                }
                crate::ChatHostStatus::Paused if host.last_transition_claim.is_some() => {
                    return Err(CoreError::ChatUnavailable(
                        crate::ChatUnavailableCause::RevivePending,
                    ));
                }
                crate::ChatHostStatus::Paused | crate::ChatHostStatus::Reviving => {
                    return self
                        .send_prompt_to_paused_chat(&caller.subject, id, text, linear)
                        .await;
                }
                crate::ChatHostStatus::Provisioning => {
                    return Err(CoreError::ChatUnavailable(
                        crate::ChatUnavailableCause::Provisioning,
                    ));
                }
                crate::ChatHostStatus::Retired => {
                    return Err(CoreError::ChatUnavailable(
                        crate::ChatUnavailableCause::Retired,
                    ));
                }
                crate::ChatHostStatus::Active
                    if host
                        .revive_failed_threads
                        .iter()
                        .any(|failure| failure.session_id == *id) =>
                {
                    let failure = host
                        .revive_failed_threads
                        .iter()
                        .find(|failure| failure.session_id == *id)
                        .expect("the active-host guard found this thread's restore failure");
                    return Err(CoreError::ChatUnavailable(
                        crate::ChatUnavailableCause::RestoreFailed {
                            phase: failure.phase,
                            retryable: failure.retryable,
                        },
                    ));
                }
                crate::ChatHostStatus::Active
                    if host.prompt_admissions.iter().any(|admission| {
                        matches!(
                            admission.state,
                            crate::ChatPromptDeliveryState::Queued
                                | crate::ChatPromptDeliveryState::DispatchFenced
                        )
                    }) =>
                {
                    return self
                        .send_prompt_to_paused_chat(&caller.subject, id, text, linear)
                        .await;
                }
                crate::ChatHostStatus::Active
                    if (record.session.status == SessionStatus::Idle
                        && record.session.stop_reason
                            == Some(crate::session::StopReason::IdleTimeout))
                        || (record.session.status == SessionStatus::Provisioning
                            && record.session.recovery_state
                                == crate::session::RecoveryState::Recovering) =>
                {
                    return self
                        .send_prompt_to_paused_chat(&caller.subject, id, text, linear)
                        .await;
                }
                crate::ChatHostStatus::Active => {}
            }
        }
        self.send_live_prompt_with_context(&caller.subject, id, text, linear)
            .await
    }

    /// Deliver one prompt after any host-level durable admission has settled.
    pub(crate) async fn send_live_prompt_with_context(
        self: &Arc<Self>,
        owner: &crate::SubjectId,
        id: &SessionId,
        text: String,
        linear: Option<LinearPromptContext>,
    ) -> Result<u64, CoreError> {
        let record = self.record(id).await?;
        if !record.session.is_owned_by(owner) {
            return Err(CoreError::Forbidden);
        }
        if let Some(state) = record.reserved_initial_prompt_state() {
            return Err(CoreError::InitialPromptReserved(state));
        }
        if record.session.status.is_terminal() {
            return Err(CoreError::SessionTerminated);
        }
        if record.is_effectively_rotating() {
            return Err(CoreError::SessionRotating);
        }
        if linear.is_some() && record.session.status == SessionStatus::Running {
            return Err(CoreError::InvalidState {
                detail: Some(CONVERSATION_IN_PROGRESS_DETAIL.into()),
            });
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
        let caller = Caller {
            subject: owner.clone(),
            authorization: crate::Authorization::from_capabilities([
                AuthorizationCapability::UseOwnSessions,
            ]),
        };
        self.conversation_input(&caller, id, Capability::Conversation)
            .await?;
        // Serialize this process's prompts before acquiring distributed
        // interaction authority. This gate participates in no lifecycle path;
        // it only preserves the established rule that a second idle prompt
        // waits to observe whether the first made steering necessary.
        let admission = self.prompt_gates.gate(id).lock_owned().await;
        let durable = self.record(id).await?;
        let effective = self.effective_prompt_text(&durable, &text).await?;
        let service = Arc::clone(self);
        let id = id.clone();
        let owner = owner.clone();
        self.run_runtime_interaction_at(&durable, move |authority| async move {
            // Compatibility stores have no durable exclusion, so retain the
            // established lifecycle-before-input order. A coordinated lease
            // already excludes lifecycle work across every replica.
            let lifecycle = if authority.is_durably_fenced() {
                None
            } else {
                Some(service.lock_lifecycle(&id).await)
            };
            let input_fence = service.input_gates.gate(&id).read_owned().await;
            let ordering = service.conversation_order_gates.gate(&id);
            let ordered_turn = ordering.lock().await;

            // Recheck the exact live generation while holding the same gates its
            // folder and lifecycle replacement use. The authority check is the
            // final synchronous step before the external delivery.
            let (
                input,
                runtime_generation,
                conversation_link,
                conversation_authority,
                runtime_id,
                running_from,
            ) = {
                let entries = service.entries.read().await;
                let entry = entries.get(&id).ok_or(CoreError::SessionNotFound)?;
                if !entry.session.is_owned_by(&owner) {
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
                if linear.is_some() && entry.session.status == SessionStatus::Running {
                    return Err(CoreError::InvalidState {
                        detail: Some(CONVERSATION_IN_PROGRESS_DETAIL.into()),
                    });
                }
                if entry.session.status == SessionStatus::Running {
                    entry.session.capabilities.require(Capability::Steer)?;
                }
                if !entry.pending_approvals.is_empty() {
                    return Err(CoreError::invalid_state());
                }
                let runtime_id = entry.runtime.as_ref().map(|runtime| runtime.0.clone());
                authority.ensure_runtime_delivery_allowed(
                    runtime_id.as_deref(),
                    entry.runtime_generation,
                )?;
                let input = entry.conversation.clone().ok_or_else(|| {
                    CoreError::RuntimeUnavailable("this session holds no conversation".into())
                })?;
                let running_from = matches!(
                    entry.session.status,
                    SessionStatus::Ready | SessionStatus::Idle
                )
                .then_some(entry.session.status);
                (
                    input,
                    entry.runtime_generation,
                    entry.conversation_link,
                    entry.conversation_authority.clone(),
                    runtime_id,
                    running_from,
                )
            };

            if let Some(context) = linear.as_ref() {
                let latest = service.latest_turn_started_unscoped(&id).await?;
                if latest != context.expected_previous_turn {
                    return Err(conversation_advanced());
                }
            }

            // A successful receipt makes the complete prompt boundary mandatory
            // finalization. Do not recheck an expiring interaction deadline
            // between delivery and enqueueing this state.
            service
                .toward_runtime(
                    &id,
                    &input,
                    ConversationInput::Prompt(vec![ContentBlock::from(effective)]),
                )
                .await?;
            let mandatory = {
                let mut entries = service.entries.write().await;
                let entry = entries
                    .get_mut(&id)
                    .filter(|entry| {
                        entry.runtime_generation == runtime_generation
                            && entry.runtime.as_ref().map(|runtime| runtime.0.as_str())
                                == runtime_id.as_deref()
                    })
                    .ok_or_else(|| {
                        CoreError::RuntimeUnavailable(
                            "the delivered prompt's runtime generation is no longer live".into(),
                        )
                    })?;
                let mut events = vec![
                    EventKind::TurnStarted,
                    EventKind::OwnerMessage { content: text },
                ];
                if let Some(context) = linear {
                    events.push(EventKind::TurnMetadata {
                        metadata: context.metadata,
                    });
                }
                let event_count = events.len() as u64;
                (append_batch(entry, events), event_count)
            };
            // Keep immediate runtime updates behind both receipt phases. The
            // mandatory history has no lifecycle ownership and can rebase over
            // a handover; the exact optional proposal below cannot.
            let (mandatory, event_count) = mandatory;
            let record = mandatory.committed().await.map_err(store_failed)?;
            let turn_started = record
                .session
                .last_seq
                .checked_sub(event_count.saturating_sub(1))
                .ok_or_else(|| {
                    CoreError::RuntimeUnavailable(
                        "session store: committed prompt boundary had no valid sequence".into(),
                    )
                })?;
            // `TurnStarted` resets approval identity during durable replay. Do
            // the same locally only after that boundary is committed, and only
            // on the exact conversation entry that delivered this prompt.
            {
                let mut entries = service.entries.write().await;
                if let Some(entry) = entries.get_mut(&id).filter(|entry| {
                    entry.runtime_generation == runtime_generation
                        && entry.conversation_link == conversation_link
                        && conversation_authority.as_ref().is_none_or(|authority| {
                            entry.conversation_authority.as_ref() == Some(authority)
                        })
                        && entry.runtime.as_ref().map(|runtime| runtime.0.as_str())
                            == runtime_id.as_deref()
                }) {
                    entry.pending_approvals.clear();
                    entry.approval_locators.clear();
                }
            }
            service
                .finish_received_history(
                    &id,
                    ReceivedHistoryTarget {
                        runtime_generation,
                        conversation_link,
                        conversation_authority,
                        runtime_id,
                        running_from,
                    },
                    record,
                )
                .await;
            drop(ordered_turn);
            drop(admission);
            drop(input_fence);
            drop(lifecycle);
            Ok(turn_started)
        })
        .await
    }

    async fn latest_turn_started_unscoped(&self, id: &SessionId) -> Result<Option<u64>, CoreError> {
        let events = self.store.events_since(id, 0).await.map_err(store_failed)?;
        Ok(events.into_iter().rev().find_map(|event| match event.kind {
            crate::StoredEventKind::Known(EventKind::TurnStarted) => Some(event.seq),
            _ => None,
        }))
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
        caller
            .authorization
            .require(AuthorizationCapability::UseOwnSessions)?;
        let record = self.record(id).await?;
        if !record.session.is_owned_by(&caller.subject) {
            return Err(CoreError::Forbidden);
        }
        if let Some(state) = record.reserved_initial_prompt_state() {
            return Err(CoreError::InitialPromptReserved(state));
        }
        // Authorization and any readoption happen before durable interaction
        // acquisition. Re-read afterwards so the lease names the exact record
        // boundary produced by that work rather than its stale input snapshot.
        self.conversation_input(caller, id, Capability::Interrupt)
            .await?;
        let record = self.record(id).await?;
        if !record.session.is_owned_by(&caller.subject) {
            return Err(CoreError::Forbidden);
        }
        if let Some(state) = record.reserved_initial_prompt_state() {
            return Err(CoreError::InitialPromptReserved(state));
        }
        if record.session.status.is_terminal() {
            return Err(CoreError::SessionTerminated);
        }
        if record.is_effectively_rotating() {
            return Err(CoreError::SessionRotating);
        }
        record.session.capabilities.require(Capability::Interrupt)?;

        let service = Arc::clone(self);
        let owner = caller.subject.clone();
        let id = id.clone();
        self.run_runtime_interaction_at(&record, move |authority| async move {
            let _lifecycle = if authority.is_durably_fenced() {
                None
            } else {
                Some(service.lock_lifecycle(&id).await)
            };
            let _input_fence = service.input_gates.gate(&id).read_owned().await;
            let (runtime_id, runtime_generation, input) = {
                let entries = service.entries.read().await;
                let entry = entries.get(&id).ok_or_else(|| {
                    CoreError::RuntimeUnavailable(
                        "this session has no live conversation entry".into(),
                    )
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
                entry.session.capabilities.require(Capability::Interrupt)?;
                let input = entry.conversation.clone().ok_or_else(|| {
                    CoreError::RuntimeUnavailable(
                        "this session holds no conversation input sender".into(),
                    )
                })?;
                (
                    entry.runtime.as_ref().map(|runtime| runtime.0.clone()),
                    entry.runtime_generation,
                    input,
                )
            };

            authority.ensure_runtime_delivery_allowed(runtime_id.as_deref(), runtime_generation)?;
            service
                .toward_runtime(&id, &input, ConversationInput::Cancel)
                .await
        })
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
            return Err(CoreError::invalid_state());
        }

        caller
            .authorization
            .require(AuthorizationCapability::UseOwnSessions)?;
        let record = self.record(id).await?;
        if !record.session.is_owned_by(&caller.subject) {
            return Err(CoreError::Forbidden);
        }
        if let Some(state) = record.reserved_initial_prompt_state() {
            return Err(CoreError::InitialPromptReserved(state));
        }

        // Readoption may itself open a runtime update stream. Complete it before
        // taking the folder's order gate, then re-read every live authority
        // under that gate rather than using the sender snapshot returned here.
        self.conversation_input(caller, id, Capability::Conversation)
            .await?;
        let durable = self.record(id).await?;
        if let Some(state) = durable.reserved_initial_prompt_state() {
            return Err(CoreError::InitialPromptReserved(state));
        }
        let service = Arc::clone(self);
        let id = id.clone();
        let owner = caller.subject.clone();
        let resolutions = resolutions.to_vec();
        self.run_runtime_interaction_at(&durable, move |authority| async move {
            let lifecycle = if authority.is_durably_fenced() {
                None
            } else {
                Some(service.lock_lifecycle(&id).await)
            };
            let input_fence = service.input_gates.gate(&id).read_owned().await;
            let ordering = service.conversation_order_gates.gate(&id);
            let ordered_resolution = ordering.lock().await;
            let (
                input,
                locators,
                runtime_id,
                runtime_generation,
                conversation_link,
                conversation_authority,
            ) = {
                let entries = service.entries.read().await;
                let entry = entries.get(&id).ok_or(CoreError::SessionNotFound)?;
                if !entry.session.is_owned_by(&owner) {
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
                        || !entry.pending_approvals.contains_key(&resolution.request_id)
                }) {
                    return Err(CoreError::invalid_state());
                }
                let input = entry.conversation.clone().ok_or_else(|| {
                    CoreError::RuntimeUnavailable("this session holds no conversation".into())
                })?;
                let locators = resolutions
                    .iter()
                    .map(|resolution| {
                        entry
                            .pending_approvals
                            .get(&resolution.request_id)
                            .cloned()
                            .expect("the batch was validated against this map")
                    })
                    .collect::<Vec<_>>();
                (
                    input,
                    locators,
                    entry.runtime.as_ref().map(|runtime| runtime.0.clone()),
                    entry.runtime_generation,
                    entry.conversation_link,
                    entry.conversation_authority.clone(),
                )
            };

            let mut delivered: usize = 0;
            let mut delivered_runtime_id = None;
            let mut delivery_error = None;
            for (resolution, locator) in resolutions.iter().zip(locators.iter().cloned()) {
                let allowed = authority
                    .ensure_runtime_delivery_allowed(runtime_id.as_deref(), runtime_generation)
                    .map(|boundary| boundary.authority.runtime_id.clone());
                let runtime_id = match allowed {
                    Ok(runtime_id) => runtime_id,
                    Err(error) => {
                        delivery_error = Some(error);
                        break;
                    }
                };
                match service
                    .toward_runtime(
                        &id,
                        &input,
                        ConversationInput::PermissionAnswer {
                            id: resolution.request_id.clone(),
                            locator,
                            decision: intent_for(resolution.decision),
                        },
                    )
                    .await
                {
                    Ok(()) => {
                        if delivered_runtime_id.is_none() {
                            delivered_runtime_id = Some(runtime_id);
                        }
                        delivered += 1;
                    }
                    Err(error) => {
                        delivery_error = Some(error);
                        break;
                    }
                }
            }

            // Whatever prefix returned receipts is mandatory finalization, even
            // when a later answer could not start because renewal authority was
            // lost. Never leave a delivered answer outstanding in the log.
            let mandatory = if delivered == 0 {
                None
            } else {
                let mut entries = service.entries.write().await;
                let entry = entries
                    .get_mut(&id)
                    .filter(|entry| {
                        entry.runtime_generation == runtime_generation
                            && entry.runtime.as_ref().map(|runtime| runtime.0.as_str())
                                == runtime_id.as_deref()
                    })
                    .ok_or_else(|| {
                        CoreError::RuntimeUnavailable(
                            "the delivered approvals' runtime generation is no longer live".into(),
                        )
                    })?;
                let mut events = Vec::with_capacity(delivered);
                for resolution in &resolutions[..delivered] {
                    events.push(EventKind::ApprovalResolved {
                        request_id: resolution.request_id.clone(),
                        allowed: matches!(resolution.decision, ApprovalDecision::Allow),
                    });
                }
                Some(append_batch(entry, events))
            };

            // Keep runtime effects behind the exact accepted prefix and its
            // optional lifecycle proposal. A later delivery error is returned
            // only after every receipt in that prefix is durable.
            if let Some(mandatory) = mandatory {
                let record = mandatory.committed().await.map_err(store_failed)?;
                let delivered_runtime_id =
                    delivered_runtime_id.expect("a delivered prefix has runtime authority");
                let running_from = {
                    let mut entries = service.entries.write().await;
                    entries
                        .get_mut(&id)
                        .filter(|entry| {
                            entry.runtime_generation == runtime_generation
                                && entry.conversation_link == conversation_link
                                && conversation_authority.as_ref().is_none_or(|authority| {
                                    entry.conversation_authority.as_ref() == Some(authority)
                                })
                                && entry.runtime.as_ref().map(|runtime| runtime.0.as_str())
                                    == delivered_runtime_id.as_deref()
                        })
                        .and_then(|entry| {
                            for (resolution, locator) in
                                resolutions[..delivered].iter().zip(&locators[..delivered])
                            {
                                if entry.pending_approvals.get(&resolution.request_id)
                                    == Some(locator)
                                {
                                    entry.pending_approvals.remove(&resolution.request_id);
                                }
                            }
                            (entry.pending_approvals.is_empty()
                                && entry.session.status == SessionStatus::Idle)
                                .then_some(SessionStatus::Idle)
                        })
                };
                service
                    .finish_received_history(
                        &id,
                        ReceivedHistoryTarget {
                            runtime_generation,
                            conversation_link,
                            conversation_authority,
                            runtime_id: delivered_runtime_id,
                            running_from,
                        },
                        record,
                    )
                    .await;
            }
            drop(ordered_resolution);
            drop(input_fence);
            drop(lifecycle);
            if let Some(error) = delivery_error {
                return Err(error);
            }
            Ok(())
        })
        .await
    }

    /// What this session has used.
    ///
    /// The sum of each runtime incarnation's highest reported totals.
    ///
    /// A runtime reports cumulative counters, so taking the maximum within one
    /// incarnation corrects a lost or temporarily lower report without double
    /// counting. A durable authority marker begins the next incarnation, whose
    /// counters start afresh; summing those per-incarnation maxima is therefore
    /// the cross-runtime total. Pre-marker history uses each durable `Rotating`
    /// transition as a legacy boundary, so existing sessions remain readable
    /// without a migration.
    pub async fn get_usage(&self, caller: &Caller, id: &SessionId) -> Result<Usage, CoreError> {
        caller
            .authorization
            .require(AuthorizationCapability::UseOwnSessions)?;
        let record = self.record(id).await?;
        if !record.session.is_owned_by(&caller.subject) {
            return Err(CoreError::Forbidden);
        }
        record.session.capabilities.require(Capability::Usage)?;
        let events = self.events_since(caller, id, None).await?;
        Ok(fold_usage(events.iter().map(|event| &event.kind)))
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
        if record.is_effectively_rotating() {
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

#[cfg(test)]
#[path = "usage_fold_tests.rs"]
mod usage_fold_tests;
