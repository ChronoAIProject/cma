use crate::{DeliveryOutcome, IntentReceipt};
use serde::{Deserialize, Serialize};
use std::ops::Deref;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum StoredReceipt {
    Paged {
        receipt: IntentReceipt,
        message_parts_pages: Box<PartsManifest>,
    },
    Inline(IntentReceipt),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartsManifest {
    pub kind: String,
    pub scope: String,
    pub owner: String,
    pub retained_until: i64,
    pub receipt_sha256: String,
    pub count: usize,
}
impl Deref for StoredReceipt {
    type Target = IntentReceipt;
    fn deref(&self) -> &IntentReceipt {
        match self {
            Self::Inline(receipt) | Self::Paged { receipt, .. } => receipt,
        }
    }
}
impl From<IntentReceipt> for StoredReceipt {
    fn from(receipt: IntentReceipt) -> Self {
        Self::Inline(receipt)
    }
}
impl StoredReceipt {
    pub fn has_parts(&self) -> bool {
        match self {
            Self::Inline(receipt) => !receipt.message_parts.is_empty(),
            Self::Paged {
                message_parts_pages,
                ..
            } => message_parts_pages.count > 0,
        }
    }

    /// A later refusal cannot erase the platform effects of earlier parts.
    pub fn requires_reconciliation(&self) -> bool {
        matches!(self.delivery, DeliveryOutcome::Unknown { .. })
            || (self.has_parts() && !matches!(self.delivery, DeliveryOutcome::Confirmed { .. }))
    }
}
