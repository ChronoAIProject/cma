//! Managed native WhatsApp Form preparation and explicit reply options.
use super::IntentReceipt;
use serde::{Deserialize, Serialize};
/// One event and selected consumer's durable preparation; this operation never sends.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct FormPreparationReceipt {
    /// Opaque lookup identity, never authorization by itself.
    pub preparation_id: String,
    /// Original event/service-window expiry in Unix seconds.
    pub expires_at: i64,
    /// Whether an explicit commit has crossed the durable send attempt boundary.
    pub customer_message: FormCustomerMessage,
    /// Durable preparation or delivery state.
    #[serde(flatten)]
    pub state: FormPreparationState,
}
/// Whether a customer message may have been attempted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormCustomerMessage {
    /// Preparation alone cannot send.
    NotSent,
    /// Consult the exact unchanged delivery receipt; never change keys to retry Unknown.
    Attempted,
}
/// Preparation progress or the explicit commit's delivery receipt.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FormPreparationState {
    /// Asset preparation is still in progress.
    Pending {
        /// Minimum suggested polling delay in milliseconds.
        retry_after_ms: u64,
    },
    /// Explicit commit is available; no customer send has occurred.
    Ready {
        /// Exact generation selector for reply_to_event_with_options.
        preparation_ref: String,
    },
    /// Preparation was declined without a customer send.
    Declined {
        /// Stable machine-readable reason.
        reason: String,
    },
    /// A managed asset mutation needs read-only reconciliation; no customer send occurred.
    Uncertain {
        /// Stable machine-readable reason.
        reason: String,
    },
    /// Original source or owner authority was withdrawn.
    Revoked {
        /// Stable machine-readable reason.
        reason: String,
    },
    /// An explicit commit crossed its send boundary.
    Delivery {
        /// Existing delivery receipt, including Unknown when unconfirmed.
        receipt: IntentReceipt,
    },
}
/// Additive options for an existing reply operation.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReplyOptions<'a> {
    /// Required stable key for a native Form commit; retain it on every retry.
    pub idempotency_key: Option<&'a str>,
    /// Opaque Ready selector; the message must exactly match preparation.
    pub form_preparation_ref: Option<&'a str>,
}
