use crate::delivery::{intents::DeliveryOutcome, retained_receipt::StoredReceipt};
use serde::{Deserialize, Serialize};

/// One bounded checkpoint per target; hashes bind the exact frame and effect.
/// Never stores a provider frame or message plaintext outside the sealed record.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryCheckpoint {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub trace_contract: bool,
    pub response_id: String,
    pub cursor: u64,
    pub frame_hash: String,
    pub effect_hash: String,
    pub terminal: bool,
    pub status: DeliveryStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<super::CustomCheckpoint>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", deny_unknown_fields)]
pub enum DeliveryStatus {
    Intent,
    Settled {
        receipt: Option<StoredReceipt>,
    },
    /// Retains confirmed parts while the aggregate stays unresolved.
    Partial {
        receipt: StoredReceipt,
    },
    Unknown,
}
impl DeliveryStatus {
    pub(super) fn settled(&self) -> bool {
        match self {
            Self::Settled { receipt: None } => true,
            Self::Settled {
                receipt: Some(receipt),
            } => !matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }),
            _ => false,
        }
    }
}
