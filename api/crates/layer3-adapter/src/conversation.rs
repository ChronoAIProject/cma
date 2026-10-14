//! ACP conversation vocabulary.
//!
//! Published schema types are re-exported directly so the adapter cannot drift
//! from the protocol spoken by the bridge.

use serde::{Deserialize, Serialize};

pub use crate::conversation_channel::{
    ConversationChannel, ConversationInputGuard, ConversationInputSender, ConversationSendOutcome,
};
pub use agent_client_protocol_schema::v1::{
    AvailableCommand, AvailableCommandsUpdate, ContentBlock, ContentChunk, MessageId,
    PermissionOption, PermissionOptionId, PermissionOptionKind, PromptResponse,
    RequestPermissionRequest, SessionId as ConversationId, SessionUpdate,
    StopReason as TurnStopReason, ToolCall, ToolCallId, ToolCallStatus, ToolCallUpdate,
    ToolCallUpdateFields, ToolKind, Usage as TurnUsage, UsageUpdate,
};

/// Opaque adapter-owned route used to answer a durable permission request.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PermissionLocator(String);

impl PermissionLocator {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for PermissionLocator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PermissionLocator(<opaque>)")
    }
}

/// A decision the agent is blocked on.
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub id: String,
    pub locator: Option<PermissionLocator>,
    pub request: RequestPermissionRequest,
}

/// What an owner decided about a permission request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow,
    Deny,
    Abort,
}

/// Input sent toward one ACP conversation.
#[derive(Debug, Clone)]
pub enum ConversationInput {
    /// Absolute live settings, acknowledged only after each ACP operation succeeds.
    Configure {
        model: String,
        reasoning_effort: crate::ReasoningEffort,
        permission: crate::PermissionLevel,
    },
    Prompt(Vec<ContentBlock>),
    RecoverablePrompt {
        content: Vec<ContentBlock>,
        prompt_id: crate::PromptIdentity,
    },
    PermissionAnswer {
        id: String,
        locator: Option<PermissionLocator>,
        decision: PermissionDecision,
    },
    Cancel,
}

/// Prompt-scoped evidence carried by an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TurnScopedEvidence {
    AgentMessage,
    ToolCallStarted,
    ToolCallUpdated,
    ApprovalRequested,
}

impl TurnScopedEvidence {
    pub const ALL: [Self; 4] = [
        Self::AgentMessage,
        Self::ToolCallStarted,
        Self::ToolCallUpdated,
        Self::ApprovalRequested,
    ];
}

/// What one adapter update proves about a prompt turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnEvidence {
    Activity(TurnScopedEvidence),
    TerminalResult,
    TerminalFailure,
}

/// One ordered update from the ACP conversation.
#[derive(Debug, Clone)]
pub enum ConversationUpdate {
    /// One ordered runtime frame. A protocol-only frame still needs a durable cursor.
    RuntimeOutput {
        receipt: crate::RuntimeOutputReceipt,
        update: Option<Box<ConversationUpdate>>,
    },
    WithoutPromptEvidence(Box<ConversationUpdate>),
    Said(SessionUpdate),
    Asked(PermissionRequest),
    TurnEnded(PromptResponse),
    Lost {
        frames: u64,
    },
    Failed {
        message: String,
        capability: Option<String>,
    },
    SignInRequired {
        url: String,
        code: Option<String>,
        message: Option<String>,
    },
}

impl ConversationUpdate {
    pub fn without_prompt_evidence(self) -> Self {
        match self {
            already @ Self::WithoutPromptEvidence(_) => already,
            update => Self::WithoutPromptEvidence(Box::new(update)),
        }
    }

    pub fn turn_evidence(&self) -> Option<TurnEvidence> {
        match self {
            Self::RuntimeOutput { update, .. } => update.as_deref().and_then(Self::turn_evidence),
            Self::WithoutPromptEvidence(_) => None,
            Self::Said(SessionUpdate::AgentMessageChunk(_)) => {
                Some(TurnEvidence::Activity(TurnScopedEvidence::AgentMessage))
            }
            Self::Said(SessionUpdate::ToolCall(_)) => {
                Some(TurnEvidence::Activity(TurnScopedEvidence::ToolCallStarted))
            }
            Self::Said(SessionUpdate::ToolCallUpdate(_)) => {
                Some(TurnEvidence::Activity(TurnScopedEvidence::ToolCallUpdated))
            }
            Self::Asked(_) => Some(TurnEvidence::Activity(
                TurnScopedEvidence::ApprovalRequested,
            )),
            Self::TurnEnded(_) => Some(TurnEvidence::TerminalResult),
            Self::Failed { .. } => Some(TurnEvidence::TerminalFailure),
            Self::Said(_) | Self::Lost { .. } | Self::SignInRequired { .. } => None,
        }
    }
}

#[cfg(test)]
#[path = "conversation_tests.rs"]
mod tests;
