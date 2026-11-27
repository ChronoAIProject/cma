//! Shared Event Gateway turn and delivery contracts.

mod custom_delivery;
mod delivery_checkpoint;
mod delivery_receipt;
mod execution_attribution;
pub mod management;
mod recovery;
mod scheduler;
mod stored_receipt;
mod turn_outcome;

pub use custom_delivery::{
    CustomCheckpoint, LocalStreamOutcome, WebhookDestination, WebhookLeg, WebhookStatus,
};
pub use delivery_checkpoint::{DeliveryCheckpoint, DeliveryStatus};
pub use delivery_receipt::{
    ConfirmedMessagePart, DeliveryOutcome, IntentReceipt, TypingOutcome, TypingReceipt,
};
pub use execution_attribution::ExecutionAttribution;
pub use recovery::{ExecutionError, Phase, RecoverySupport};
pub use scheduler::{Disposition, ScheduleConfig, ScheduleReport, WorkSelection};
pub use stored_receipt::{PartsManifest, StoredReceipt};
pub use turn_outcome::TurnOutcome;

/// Stable response classification read by Gateway request telemetry.
#[derive(Clone)]
pub struct RefusalReason(pub String);
