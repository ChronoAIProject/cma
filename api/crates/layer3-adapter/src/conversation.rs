//! ACP conversation vocabulary.
//!
//! Published schema types are re-exported directly so the adapter cannot drift
//! from the protocol spoken by the bridge.

use serde::{Deserialize, Serialize};

pub use crate::conversation_channel::{
    ConversationChannel, ConversationInputGuard, ConversationInputSender, ConversationSendOutcome,
    ConversationSettingsSupport,
};
pub use agent_client_protocol_schema::v1::{
    AvailableCommand, AvailableCommandsUpdate, ContentBlock, ContentChunk, MessageId,
    PermissionOption, PermissionOptionId, PermissionOptionKind, PromptResponse,
    RequestPermissionRequest, SessionId as ConversationId, SessionUpdate,
    StopReason as TurnStopReason, ToolCall, ToolCallContent, ToolCallId, ToolCallStatus,
    ToolCallUpdate, ToolCallUpdateFields, ToolKind, Usage as TurnUsage, UsageUpdate,
};

pub fn reported_tool_text(fields: &ToolCallUpdateFields) -> Vec<std::borrow::Cow<'_, str>> {
    if let Some(summary) = crate::codex::deliverable_output::failure_summary(
        fields.status.as_ref(),
        fields.raw_input.as_ref(),
        fields.raw_output.as_ref(),
    ) {
        return vec![summary];
    }
    reported_tool_text_parts(fields.content.as_deref(), fields.raw_output.as_ref())
}

pub fn reported_tool_call_text(call: &ToolCall) -> Vec<std::borrow::Cow<'_, str>> {
    if let Some(summary) = crate::codex::deliverable_output::failure_summary(
        Some(&call.status),
        call.raw_input.as_ref(),
        call.raw_output.as_ref(),
    ) {
        return vec![summary];
    }
    reported_tool_text_parts(Some(&call.content), call.raw_output.as_ref())
}

fn reported_tool_text_parts<'a>(
    content: Option<&'a [ToolCallContent]>,
    raw_output: Option<&'a serde_json::Value>,
) -> Vec<std::borrow::Cow<'a, str>> {
    if let Some(content) = content {
        let diffs = crate::tool_diff::reported_diffs(content).map(std::borrow::Cow::Owned);
        return diffs
            .into_iter()
            .chain(content.iter().filter_map(|item| match item {
                ToolCallContent::Content(block) => match &block.content {
                    ContentBlock::Text(block) => {
                        Some(std::borrow::Cow::Borrowed(block.text.as_str()))
                    }
                    _ => None,
                },
                _ => None,
            }))
            .collect();
    }
    raw_output
        .and_then(|output| output.get("formatted_output"))
        .and_then(serde_json::Value::as_str)
        .into_iter()
        .map(std::borrow::Cow::Borrowed)
        .collect()
}

/// File edits and historical terminal failures can arrive in the initial call.
pub fn has_reported_initial_output(call: &ToolCall) -> bool {
    call.content
        .iter()
        .any(|content| matches!(content, ToolCallContent::Diff(_)))
        || crate::codex::deliverable_output::is_failed_deliverable(
            Some(&call.status),
            call.raw_input.as_ref(),
        )
}

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
        system_prompt: Option<String>,
    },
    RefreshContext {
        service_origin: Option<String>,
        operation_sequence: u64,
        readiness_receipt: String,
        previous_guidance: Vec<String>,
        working_directory: Option<String>,
        guidance: String,
        model: String,
        reasoning_effort: crate::ReasoningEffort,
        permission: crate::PermissionLevel,
    },
    Prompt(Vec<ContentBlock>),
    /// Inject input into the current native turn without starting another prompt.
    Steer(Vec<ContentBlock>),
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
    /// Immutable domain storage whose reservation follows every shared owner.
    RetainedEffect(crate::RuntimeOutputUpdate),
    /// One ordered runtime frame. A protocol-only frame still needs a durable cursor.
    RuntimeOutput {
        receipt: crate::RuntimeOutputReceipt,
        update: Option<crate::RuntimeOutputUpdate>,
    },
    WithoutPromptEvidence(Box<ConversationUpdate>),
    Said(SessionUpdate),
    Metadata(crate::ConversationMetadata),
    Usage(crate::ConversationUsage),
    Asked(PermissionRequest),
    TurnEnded(PromptResponse),
    /// A terminal response correlated to one already admitted prompt.
    TurnEndedForPrompt {
        prompt_id: crate::PromptIdentity,
        response: PromptResponse,
    },
    Lost {
        frames: u64,
    },
    Failed {
        message: String,
        capability: Option<String>,
    },
    /// A terminal failure correlated to one already admitted prompt.
    FailedForPrompt {
        prompt_id: crate::PromptIdentity,
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
    /// Borrow the complete delivery even through an immutable outer envelope.
    pub fn delivery(&self) -> crate::runtime_output::RuntimeOutputDelivery<'_> {
        match self {
            Self::RetainedEffect(update) => update.delivery(),
            Self::RuntimeOutput { receipt, update } => {
                crate::runtime_output::RuntimeOutputDelivery {
                    receipt: Some(receipt),
                    effect: update.as_deref().map(Self::effect),
                }
            }
            update => crate::runtime_output::RuntimeOutputDelivery {
                receipt: None,
                effect: Some(update),
            },
        }
    }

    /// Borrow a domain effect without separating it from its storage owner.
    pub fn effect(&self) -> &Self {
        match self {
            Self::RetainedEffect(update) => update.effect(),
            update => update,
        }
    }

    pub fn without_prompt_evidence(self) -> Self {
        match self {
            already @ Self::WithoutPromptEvidence(_) => already,
            update => Self::WithoutPromptEvidence(Box::new(update)),
        }
    }

    pub fn turn_evidence(&self) -> Option<TurnEvidence> {
        match self {
            Self::RetainedEffect(update) => update.turn_evidence(),
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
            Self::TurnEndedForPrompt { .. } => Some(TurnEvidence::TerminalResult),
            Self::Failed { .. } => Some(TurnEvidence::TerminalFailure),
            Self::FailedForPrompt { .. } => Some(TurnEvidence::TerminalFailure),
            Self::Said(_)
            | Self::Metadata(_)
            | Self::Usage(_)
            | Self::Lost { .. }
            | Self::SignInRequired { .. } => None,
        }
    }
}

#[cfg(test)]
#[path = "conversation_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "deliverable_output_tests.rs"]
mod deliverable_output_tests;
