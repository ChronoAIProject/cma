//! Reserved ordinary-text protocol. Classification grants no Agent authority.
use serde::{Deserialize, Serialize};

use super::{InboxReceipt, SignedCallback};
use crate::{bridge::BridgeId, sandbox::agent_io::AgentInput};

const PREFIX: &str = "CMA-CONTROL";
const VERSION: &str = "CMA-CONTROL/1";
const MAX_CONTROL_BYTES: usize = 65_536;

/// A transport outcome, never an Agent command receipt or cancellation result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "receipt",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CallbackAcceptance {
    Message(InboxReceipt),
    ControlRefused(ChannelControlReceipt),
    ControlAccepted(ChannelControlAcceptedReceipt),
}

impl CallbackAcceptance {
    pub fn into_message(self) -> Option<InboxReceipt> {
        match self {
            Self::Message(receipt) => Some(receipt),
            Self::ControlRefused(_) | Self::ControlAccepted(_) => None,
        }
    }

    pub fn callback_id(&self) -> &str {
        match self {
            Self::Message(receipt) => &receipt.inbox_id,
            Self::ControlRefused(receipt) => &receipt.callback_id,
            Self::ControlAccepted(receipt) => &receipt.callback_id,
        }
    }
}

/// Version-two acknowledgement of a durably retained control intent. This is
/// not evidence that a command was dispatched or that its operation completed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelControlAcceptedReceipt {
    pub version: u16,
    pub callback_id: String,
    pub bridge_id: BridgeId,
    pub message_id: String,
    pub intent_id: String,
    pub persisted_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelControlReceipt {
    pub version: u16,
    pub callback_id: String,
    pub bridge_id: BridgeId,
    pub message_id: String,
    pub persisted_at: i64,
    pub code: ChannelControlRefusal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelControlRefusal {
    ChannelControlsDisabled,
    InvalidChannelControlRequest,
    ChannelControlVersionUnsupported,
    ChannelControlOperationUnsupported,
    ApprovalDashboardOnly,
}

impl SignedCallback {
    /// Called only after trusted raw identity/text classification and admission.
    pub(super) fn classify_reserved_control(&mut self) {
        self.control_refusal = self.record.envelope.text.as_deref().and_then(classify);
    }
}

/// Reserve the case-sensitive namespace at the start, ignoring leading
/// whitespace only for reservation. Malformed spacing is refused, not executed.
/// Embedded examples and ordinary chat text have no control interpretation.
fn classify(text: &str) -> Option<ChannelControlRefusal> {
    use ChannelControlRefusal as R;
    if !text.trim_start().starts_with(PREFIX) {
        return None;
    }
    if text.len() > MAX_CONTROL_BYTES || text.trim_start() != text {
        return Some(R::InvalidChannelControlRequest);
    }
    let (header, payload) = text
        .split_once('\n')
        .map_or((text, None), |(h, p)| (h, Some(p)));
    let mut words = header.split(' ');
    let version = words.next().unwrap_or_default();
    if version != VERSION {
        return Some(
            if version
                .strip_prefix("CMA-CONTROL/")
                .is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
            {
                R::ChannelControlVersionUnsupported
            } else {
                R::InvalidChannelControlRequest
            },
        );
    }
    let operation = words.next().unwrap_or_default();
    if matches!(
        operation,
        "approve" | "approval" | "allow" | "deny" | "abort"
    ) {
        return Some(R::ApprovalDashboardOnly);
    }
    if !matches!(operation, "cancel" | "steer") {
        return Some(if operation.is_empty() {
            R::InvalidChannelControlRequest
        } else {
            R::ChannelControlOperationUnsupported
        });
    }
    let reference = words.next().unwrap_or_default();
    let valid_reference = reference.strip_prefix("ccr_").is_some_and(|value| {
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    });
    let valid_payload = match (operation, payload) {
        ("cancel", None) => true,
        ("steer", Some(text)) => AgentInput::text(text).canonical().is_ok(),
        _ => false,
    };
    Some(
        if words.next().is_some() || !valid_reference || !valid_payload {
            R::InvalidChannelControlRequest
        } else {
            R::ChannelControlsDisabled
        },
    )
}

#[cfg(test)]
mod tests;
