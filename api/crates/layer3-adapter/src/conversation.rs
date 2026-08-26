//! The conversation plane's vocabulary.
//!
//! Split from the trait so that file stays within the project's size limit, on
//! a real division: the trait says what a kernel can be asked to do, and this
//! says what a conversation is made of.
//!
//! Most of it is the Agent Client Protocol's own, re-exported rather than
//! re-declared. Re-declaring would mean maintaining a copy of a published
//! schema, and a copy is wrong from the first release that moves.

use std::future::Future;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

use crate::AdapterError;

/// How long a local adapter has to deliver one operation to its runtime link.
///
/// This is not a network response deadline. It bounds the handoff to the task
/// already holding the link, which is either immediate or is evidence that
/// task is no longer able to receive work.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(2);

/// The protocol's own types, re-exported rather than re-declared.
///
/// Re-declaring them would mean maintaining a copy of a published schema, and
/// a copy is wrong from the first release that moves. `StopReason` is renamed
/// on the way through: the core has a stop reason of its own about whole
/// sessions, and these five are about a single turn.
pub use agent_client_protocol_schema::v1::{
    AvailableCommand, AvailableCommandsUpdate, ContentBlock, ContentChunk, PermissionOption,
    PermissionOptionId, PermissionOptionKind, PromptResponse, RequestPermissionRequest,
    SessionId as ConversationId, SessionUpdate, StopReason as TurnStopReason, ToolCall, ToolCallId,
    ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind, UsageUpdate,
};
pub use agent_client_protocol_schema::v1::{MessageId, Usage as TurnUsage};

/// Opaque adapter-owned information needed to deliver a permission answer.
///
/// The core records and returns this value without interpreting it. That lets
/// an adapter reconnect after a control-plane replacement without teaching
/// the durable layer its wire protocol, while keeping the tool payload and
/// every credential out of history.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PermissionLocator(String);

impl PermissionLocator {
    /// Wrap an adapter's versioned locator representation.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the opaque representation at the adapter boundary that owns it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for PermissionLocator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PermissionLocator(<opaque>)")
    }
}

/// A decision the runtime is blocked on, and the handle to answer it with.
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    /// Correlates an answer with the request that is waiting for it.
    ///
    /// Minted by the adapter. The protocol carries this at the JSON-RPC layer
    /// rather than in the payload, and the core needs something it can record
    /// against an approval event, hand to whoever is being asked, and accept
    /// back from them minutes later — possibly after a restart, and possibly
    /// through a different interface than the one that showed it.
    pub id: String,
    /// Adapter-owned delivery information that survives a control-plane
    /// replacement.
    ///
    /// Absent for adapters that can answer from the public id alone, and for
    /// requests produced by builds predating durable delivery locators.
    pub locator: Option<PermissionLocator>,
    /// What the runtime is asking to do, and the answers it will accept.
    ///
    /// The offered options come from the runtime rather than from here. A
    /// fixed allow/deny pair invented at this layer would drop the choices a
    /// kernel actually distinguishes — refusing one command and refusing the
    /// whole turn are different answers, and only the kernel knows which of
    /// them it is offering.
    pub request: RequestPermissionRequest,
}

/// Something sent toward a runtime's conversation.
#[derive(Debug, Clone)]
pub enum ConversationInput {
    /// Content that opens a prompt turn.
    Prompt(Vec<ContentBlock>),
    /// The answer to a permission request the runtime is blocked on.
    PermissionAnswer {
        /// The request being answered, as [`PermissionRequest::id`] named it.
        id: String,
        /// The durable adapter-owned route recorded with the request.
        locator: Option<PermissionLocator>,
        /// What the owner decided.
        ///
        /// An intent rather than one of the runtime's own options, on the same
        /// terms as [`RuntimeOptions`]: the caller says what was decided and
        /// the adapter renders it into whatever this runtime offered. Only the
        /// adapter knows what those options meant, and a caller choosing one by
        /// name would have to learn a vocabulary that differs per kernel and
        /// changes between their versions.
        decision: PermissionDecision,
    },
    /// Abandon the turn in flight.
    ///
    /// The turn ends; the conversation does not. A runtime that treats this as
    /// a reason to stop has lost the session, which is the distinction between
    /// interrupting and terminating.
    Cancel,
}

/// Input queued together with the receipt its caller awaits.
struct PendingInput {
    input: ConversationInput,
    delivered: oneshot::Sender<Result<(), AdapterError>>,
}

/// The sending half of a runtime conversation.
///
/// A Tokio sender confirms only that a queue slot accepted a value. It says
/// nothing about whether the task behind that queue is still reading, or
/// whether its runtime link can take the frame. This sender waits for the
/// adapter task to report that result.
#[derive(Clone)]
pub struct ConversationInputSender {
    inner: mpsc::Sender<PendingInput>,
}

impl std::fmt::Debug for ConversationInputSender {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConversationInputSender")
            .field("closed", &self.inner.is_closed())
            .field("capacity", &self.inner.capacity())
            .finish()
    }
}

impl ConversationInputSender {
    /// Run the consumer for a conversation input channel.
    ///
    /// The handler returns after writing the operation to the runtime link,
    /// not after a long-lived turn has finished.
    pub fn spawn<F, Fut>(capacity: usize, handler: F) -> Self
    where
        F: FnMut(ConversationInput) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), AdapterError>> + Send + 'static,
    {
        Self::spawn_with_abort(capacity, handler).0
    }

    /// Send one operation and wait until the adapter has delivered it.
    pub async fn send(&self, input: ConversationInput) -> Result<(), AdapterError> {
        let (delivered, receipt) = oneshot::channel();
        self.inner
            .try_send(PendingInput { input, delivered })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => AdapterError::Unavailable(
                    "the conversation input queue is not being consumed".to_string(),
                ),
                mpsc::error::TrySendError::Closed(_) => {
                    AdapterError::Unavailable("the conversation input task has ended".to_string())
                }
            })?;

        tokio::time::timeout(DELIVERY_DEADLINE, receipt)
            .await
            .map_err(|_| {
                AdapterError::Unavailable(
                    "the conversation input task did not deliver the operation".to_string(),
                )
            })?
            .map_err(|_| {
                AdapterError::Unavailable(
                    "the conversation input task ended before delivery".to_string(),
                )
            })?
    }

    /// Hosted adapters stop this consumer when the socket's read half ends.
    pub(crate) fn spawn_with_abort<F, Fut>(
        capacity: usize,
        mut handler: F,
    ) -> (Self, tokio::task::AbortHandle)
    where
        F: FnMut(ConversationInput) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), AdapterError>> + Send + 'static,
    {
        let (inner, mut pending) = mpsc::channel::<PendingInput>(capacity);
        let task = tokio::spawn(async move {
            while let Some(PendingInput { input, delivered }) = pending.recv().await {
                let _ = delivered.send(handler(input).await);
            }
        });
        let abort = task.abort_handle();
        (Self { inner }, abort)
    }
}

/// What an owner decided about something a runtime asked to do.
///
/// Three answers rather than two, because refusing one action and abandoning
/// what the agent was doing are different intentions. A person who meant the
/// second and got the first watches the agent try something else instead of
/// stopping, which is the opposite of what they asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Let it proceed.
    Allow,
    /// Refuse it. The turn carries on without doing this.
    Deny,
    /// Refuse it and stop the turn.
    Abort,
}

/// One thing that happened on a runtime's conversation.
///
/// Everything travels in one stream, in the order it happened, because that is
/// what a conversation is and what the durable log has to be. Split across
/// several streams, a turn ending could overtake the last words of the message
/// it was ending, and a permission request could be recorded before the tool
/// call that provoked it. Both would be recorded wrong, permanently, and
/// neither would be visible until somebody read the history back.
#[derive(Debug, Clone)]
pub enum ConversationUpdate {
    /// Something the runtime said, in the protocol's own words.
    Said(SessionUpdate),
    /// A decision the runtime is now blocked on.
    Asked(PermissionRequest),
    /// The prompt turn in flight ended, and why.
    TurnEnded(PromptResponse),
    /// Part of what the runtime said was dropped rather than delivered.
    ///
    /// Happens when it held more than it could while nothing was listening —
    /// a long control-plane restart — and had to drop the oldest of it.
    Lost {
        /// How many frames went, as the runtime counted them.
        frames: u64,
    },
    /// The turn stopped because something went wrong, and what.
    ///
    /// Distinct from `TurnEnded` deliberately. A turn that ends is a turn the
    /// agent finished; this one did not, and the difference is the whole of
    /// what a client needs to tell "the agent had nothing more to say" from
    /// "the agent could not answer". Reported so it becomes `RUN_ERROR`
    /// rather than a turn that quietly produced nothing — the failure
    /// `docs/API_AGUI.md` requires a client be able to see, and the one
    /// `CLAUDE.md` §9 forbids being discovered by whoever tried to use it.
    Failed {
        /// What went wrong, in the words of whatever knew: the agent's own
        /// message, the bridge's, or the runtime's.
        message: String,
        /// The ability that was missing, where the failure was a refusal.
        capability: Option<String>,
    },
    /// The agent cannot proceed until its owner signs in to their own account.
    ///
    /// Only ever reached by a session created with the owner's own credential.
    /// The kernel is asking a person to complete a sign-in somewhere else, and
    /// the two things it needs them to know — where to go and what to type —
    /// travel here so an interface can put them on screen.
    ///
    /// **No credential passes this way, in either direction.** What the sign-in
    /// produces is written inside the sandbox by the kernel itself; this
    /// carries only the address of a page and a code that is useless without
    /// the person's own account. That is what makes the promise in
    /// `CredentialMode::Own` true rather than aspirational.
    SignInRequired {
        /// The page the owner opens, in their own browser, on their own device.
        url: String,
        /// The one-time code they enter there. Short-lived by construction.
        code: Option<String>,
        /// The kernel's own sentence about what it wants, where it gave one.
        message: Option<String>,
    },
}

/// A bidirectional attachment to a runtime's conversation.
///
/// One stream each way. Outward it is ordered, because the durable log built
/// from it is ordered and nothing downstream can put back a sequence that was
/// scrambled here. Inward it is separate because it is driven by a person
/// rather than by the runtime, and the two have nothing to say about each
/// other's pace.
#[derive(Debug)]
pub struct ConversationChannel {
    /// Input destined for the runtime.
    pub input: ConversationInputSender,
    /// What the runtime calls this conversation, when it has a name for one.
    ///
    /// Reported so it can be recorded, because a control plane that restarts
    /// remembers nothing and the alternative is opening a second conversation
    /// beside the one already running — the agent loses its context while the
    /// owner, reading a durable log that has everything, cannot tell.
    ///
    /// `None` from a runtime whose conversations have no name. Such a runtime
    /// cannot be rejoined either, and says so by refusing to be asked.
    pub conversation_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_queue_slot_without_a_draining_consumer_is_not_delivery() {
        // This is the shape that used to produce a false success: the receiver
        // is alive, so Tokio accepts the value, but no task ever reads it.
        let (inner, _held_open) = mpsc::channel(1);
        let input = ConversationInputSender { inner };

        let error = input
            .send(ConversationInput::Cancel)
            .await
            .expect_err("an undrained queue did not deliver an interrupt");

        assert!(matches!(error, AdapterError::Unavailable(message)
            if message.contains("did not deliver")));
    }
}
