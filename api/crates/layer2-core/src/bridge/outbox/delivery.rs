use async_trait::async_trait;

use crate::bridge::{BridgeRecord, BridgeValidation};

use super::*;

/// Trusted adapter observation bound to this exact original source. Capability
/// knowledge must come from the pinned connector, never model/callback metadata.
pub struct DeliveryCapabilityObservation {
    pub source_fingerprint: String,
    pub original_route_supported: bool,
    pub edit_supported: bool,
    pub deadline: tokio::time::Instant,
}

/// No serialization, Debug or Clone. Construct only after a Sending transition
/// committed. Its existence does not replace the adapter's current secret and
/// token checks; credential references are never mounted in an Agent runtime.
pub struct RelayDeliveryRequest {
    item: OutboxItem,
    validation: BridgeValidation,
    deadline: tokio::time::Instant,
    edit_original: Option<DeliveryReceipt>,
}
impl RelayDeliveryRequest {
    pub fn after_sending_commit(
        item: OutboxItem,
        bridge: &BridgeRecord,
        validation: BridgeValidation,
        capabilities: DeliveryCapabilityObservation,
        now_ms: i64,
    ) -> Result<Self, OutboxError> {
        item.validate()?;
        item.source.require_bridge(bridge)?;
        item.credential.require_source(&item.source, now_ms)?;
        let DeliveryState::Sending { expires_at_ms, .. } = item.state else {
            return Err(OutboxError::Conflict);
        };
        if expires_at_ms <= now_ms || capabilities.deadline <= tokio::time::Instant::now() {
            return Err(OutboxError::Expired);
        }
        validation
            .require(&item.source.owner, item.source.config())
            .map_err(|_| OutboxError::AuthorityUnavailable)?;
        if let DeliveryCredentialRef::RetainedKey {
            reference,
            revision,
            key_state_version,
        } = &item.credential
        {
            let custody = validation.custody_pin();
            if reference != custody.reference()
                || *revision != custody.revision()
                || *key_state_version != custody.key_state_version()
            {
                return Err(OutboxError::AuthorityUnavailable);
            }
        }
        if capabilities.source_fingerprint != item.source.fingerprint()?
            || !capabilities.original_route_supported
            || (matches!(item.operation, RelayOperation::Edit { .. })
                && !capabilities.edit_supported)
        {
            return Err(OutboxError::CapabilityUnavailable);
        }
        let credential_deadline = match &item.credential {
            DeliveryCredentialRef::ReplyToken { expires_at_ms, .. } => *expires_at_ms,
            DeliveryCredentialRef::RetainedKey { .. } => i64::MAX,
        };
        let stop_at = expires_at_ms
            .min(credential_deadline)
            .min(item.source.route_expires_at_ms.unwrap_or(i64::MAX));
        let remaining = u64::try_from(stop_at - now_ms).map_err(|_| OutboxError::Expired)?;
        let deadline = capabilities
            .deadline
            .min(validation.deadline())
            .min(tokio::time::Instant::now() + std::time::Duration::from_millis(remaining));
        Ok(Self {
            item,
            validation,
            deadline,
            edit_original: None,
        })
    }
    pub fn with_edit_original(mut self, original: &OutboxItem) -> Result<Self, OutboxError> {
        self.edit_original = Some(self.item.require_edit_original(original)?.clone());
        self.require_current()?;
        Ok(self)
    }
    pub fn edit_original_receipt(&self) -> Option<&DeliveryReceipt> {
        self.edit_original.as_ref()
    }
    pub fn item(&self) -> &OutboxItem {
        &self.item
    }
    pub fn deadline(&self) -> tokio::time::Instant {
        self.deadline
    }
    pub fn require_current(&self) -> Result<(), OutboxError> {
        if matches!(self.item.operation, RelayOperation::Edit { .. })
            && self.edit_original.is_none()
        {
            return Err(OutboxError::SourceMismatch);
        }
        if tokio::time::Instant::now() >= self.deadline {
            return Err(OutboxError::Expired);
        }
        self.validation
            .require(&self.item.source.owner, self.item.source.config())
            .map_err(|_| OutboxError::AuthorityUnavailable)
    }
    pub fn path(&self) -> &'static str {
        match self.item.operation {
            RelayOperation::InitialReply => "/api/v1/channel-relay/reply",
            RelayOperation::Edit { .. } => "/api/v1/channel-relay/reply/update",
        }
    }
    /// The original message chooses the recipient. No chat, bot, thread, URL,
    /// mentions, card or attachment metadata can be supplied by output text.
    pub fn body(&self) -> serde_json::Value {
        let message_id = match &self.item.operation {
            RelayOperation::InitialReply => self.item.source.message_id(),
            RelayOperation::Edit {
                platform_message_id,
                ..
            } => platform_message_id,
        };
        serde_json::json!({"message_id":message_id,"reply":{"text":self.item.text()}})
    }
}

/// These are positive no-send observations. An HTTP 5xx, timeout, provider error
/// or consumed-token conflict is insufficient evidence for either variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotSentEvidence {
    NoRequestBytesWritten,
    VerifiedRejectionBeforeTokenConsumption,
}

/// Broad telemetry receives only these bounded codes, never provider bodies,
/// outbound text, source chat/thread identities or secret-bearing URLs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryOutcome {
    Confirmed(DeliveryReceipt),
    DefinitiveRejection(DeliveryFailure),
    RetryableNotSent {
        reason: DeliveryFailure,
        retry_at_ms: i64,
        evidence: NotSentEvidence,
    },
    Unknown(DeliveryFailure),
    /// The provider reported throttling, but did not prove that sending was
    /// impossible. The original attempt stays Unknown. The timestamp only
    /// delays subsequent sends through the same bot.
    RateLimitedUnknown {
        not_before_ms: i64,
    },
}

#[async_trait]
pub trait RelayDeliveryPort: Send + Sync {
    /// One bounded request to the deployment-pinned NyxID endpoint. Recheck
    /// exact stored Sending/attempt, source, current Bridge controls and actual
    /// selected encrypted custody; verify reply-token signature/expiry/binding
    /// and original interaction deadline. Use the original retained key only.
    /// Never follow redirects, automatically retry, or fall back to a direct
    /// platform client. A key ID or this request alone supplies no authority.
    ///
    /// NyxID consumes reply-token JTI before validation/send and persists reply
    /// metadata afterward. Any result that cannot prove no send or confirmed
    /// success is Unknown, including parse failure after a successful response.
    async fn deliver(&self, request: RelayDeliveryRequest) -> DeliveryOutcome;
}
