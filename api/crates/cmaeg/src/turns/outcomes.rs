//! Events passed to delivery, retaining context only for the lifetime of a turn.
pub use crate::delivery::intents::DeliveryCommand;
use crate::delivery::intents::{DeliveryOutcome, IntentReceipt, TypingOutcome, TypingReceipt};
use cmaeg_core::message::{RenderingOutcome, RenderingReport};
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
/// Temporary production sink until the delivery engine is composed. Never log content or refs.
pub async fn log_outcomes(mut receiver: mpsc::Receiver<DeliveryCommand>) {
    while let Some(command) = receiver.recv().await {
        match command {
            DeliveryCommand::StreamOutcome { outcome, .. } => {
                tracing::info!(outcome = outcome.kind(), "provider turn outcome")
            }
            DeliveryCommand::Typing { receipt, .. } => {
                let _ = receipt.send(TypingReceipt {
                    outcome: TypingOutcome::Unsupported,
                });
            }
            DeliveryCommand::Update { receipt, .. } => {
                let _ = receipt.send(IntentReceipt {
                    message_ref: None,
                    rendering: RenderingReport {
                        outcome: RenderingOutcome::Unsupported,
                        blocks: vec![],
                    },
                    delivery: DeliveryOutcome::Unsupported,
                });
            }
            DeliveryCommand::Reply { receipt, .. } => {
                let _ = receipt.send(IntentReceipt {
                    message_ref: None,
                    rendering: RenderingReport {
                        outcome: RenderingOutcome::Unsupported,
                        blocks: vec![],
                    },
                    delivery: DeliveryOutcome::Rejected {
                        reason: "delivery_not_configured".into(),
                    },
                });
            }
        }
    }
}
