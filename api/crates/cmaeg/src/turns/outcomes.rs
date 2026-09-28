//! Events passed to delivery, retaining context only for the lifetime of a turn.
pub use crate::delivery::intents::DeliveryCommand;
use cmaeg_core::message::OutboundMessage;

#[derive(Debug, Clone, PartialEq)]
pub enum TurnOutcome {
    Accepted { response_id: String },
    TextDelta { text: String },
    MessageCommitted { text: String },
    ToolStarted { name: String },
    ToolFinished { name: String, ok: bool },
    Completed { final_text: Option<String> },
    Failed { code: String, message: String },
    Incomplete { reason: String },
    SetupMessage { message: OutboundMessage },
}
impl TurnOutcome {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Accepted { .. } => "accepted",
            Self::TextDelta { .. } => "text_delta",
            Self::MessageCommitted { .. } => "message_committed",
            Self::ToolStarted { .. } => "tool_started",
            Self::ToolFinished { .. } => "tool_finished",
            Self::Completed { .. } => "completed",
            Self::Failed { .. } => "failed",
            Self::Incomplete { .. } => "incomplete",
            Self::SetupMessage { .. } => "setup_message",
        }
    }
    pub fn terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed { .. }
                | Self::Failed { .. }
                | Self::Incomplete { .. }
                | Self::SetupMessage { .. }
        )
    }
}
