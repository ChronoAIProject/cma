//! Shared Event Gateway turn and delivery contracts.

mod delivery_receipt;
mod turn_outcome;

pub use delivery_receipt::{
    ConfirmedMessagePart, DeliveryOutcome, IntentReceipt, TypingOutcome, TypingReceipt,
};
pub use turn_outcome::TurnOutcome;

/// Stable response classification read by Gateway request telemetry.
#[derive(Clone)]
pub struct RefusalReason(pub String);
