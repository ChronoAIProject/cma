//! The conversation plane's vocabulary.
//!
//! Split from the trait so that file stays within the project's size limit, on
//! a real division: the trait says what a kernel can be asked to do, and this
//! says what a conversation is made of.
//!
//! Most of it is the Agent Client Protocol's own, re-exported rather than
//! re-declared. Re-declaring would mean maintaining a copy of a published
//! schema, and a copy is wrong from the first release that moves.

use tokio::sync::mpsc;

/// The protocol's own types, re-exported rather than re-declared.
///
/// Re-declaring them would mean maintaining a copy of a published schema, and
/// a copy is wrong from the first release that moves. `StopReason` is renamed
/// on the way through: the core has a stop reason of its own about whole
/// sessions, and these five are about a single turn.
pub use agent_client_protocol_schema::v1::{
    ContentBlock, ContentChunk, PermissionOption, PermissionOptionId, PermissionOptionKind,
    PromptResponse, RequestPermissionRequest, SessionId as ConversationId, SessionUpdate,
    StopReason as TurnStopReason, ToolCall, ToolCallId, ToolCallStatus, ToolCallUpdate,
    ToolCallUpdateFields, ToolKind, UsageUpdate,
};
pub use agent_client_protocol_schema::v1::{MessageId, Usage as TurnUsage};

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
    /// Everything that happens on the conversation, in the order it happens.
    pub updates: mpsc::Receiver<ConversationUpdate>,
    /// Input destined for the runtime.
    pub input: mpsc::Sender<ConversationInput>,
}
