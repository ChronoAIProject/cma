use crate::{CustomCheckpoint, DeliveryOutcome, ExecutionAttribution, StoredReceipt};
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
    /// Verified response-scoped evidence remains available after cursor acknowledgement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_attribution: Option<ExecutionAttribution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<CustomCheckpoint>,
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
    pub fn settled(&self) -> bool {
        match self {
            Self::Settled { receipt: None } => true,
            Self::Settled {
                receipt: Some(receipt),
            } => !matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }),
            _ => false,
        }
    }
}
