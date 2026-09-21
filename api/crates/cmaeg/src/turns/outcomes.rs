//! Events passed to delivery, retaining context only for the lifetime of a turn.
use crate::ingress::launch::LaunchRequest;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    Accepted { response_id: String },
    TextDelta { text: String },
    MessageCommitted { text: String },
    ToolStarted { name: String },
    ToolFinished { name: String, ok: bool },
    Completed { final_text: Option<String> },
    Failed { code: String, message: String },
    Incomplete { reason: String },
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
        }
    }
    pub fn terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed { .. } | Self::Failed { .. } | Self::Incomplete { .. }
        )
    }
}
#[derive(Debug, Clone)]
pub struct DeliveryCommand {
    pub turn: Arc<LaunchRequest>,
    pub outcome: TurnOutcome,
}
/// Temporary production sink until the delivery engine is composed. Never log content or refs.
pub async fn log_outcomes(mut receiver: mpsc::Receiver<DeliveryCommand>) {
    while let Some(command) = receiver.recv().await {
        tracing::info!(outcome = command.outcome.kind(), "provider turn outcome");
    }
}
