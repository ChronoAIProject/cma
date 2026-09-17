use serde::{Deserialize, Serialize};

use crate::bridge::ingress::{InboxAdmission, StoredInbox};
use crate::bridge::{BridgeConfig, BridgeId, BridgePlatform, BridgeRecord, BridgeWorkPin};
use crate::resource_access::ResourceOwner;

use super::{opaque_id, PartitionError};

mod observation;
pub use observation::*;

/// Correlation only, never evidence of a current grant or custody permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PartitionSourcePin {
    inbox_id: String,
    body_sha256: String,
    work_pin: BridgeWorkPin,
}

impl PartitionSourcePin {
    pub fn for_inbox(inbox: &StoredInbox) -> Result<Self, PartitionError> {
        let value = Self {
            inbox_id: inbox.id.clone(),
            body_sha256: inbox.attestation.body_sha256.clone(),
            work_pin: inbox.pin.clone(),
        };
        value.validate()?;
        Ok(value)
    }
    pub fn inbox_id(&self) -> &str {
        &self.inbox_id
    }
    pub fn body_sha256(&self) -> &str {
        &self.body_sha256
    }
    pub fn work_pin(&self) -> &BridgeWorkPin {
        &self.work_pin
    }
    fn validate(&self) -> Result<(), PartitionError> {
        if !super::digest(&self.inbox_id)
            || !super::digest(&self.body_sha256)
            || !opaque_id(self.work_pin.bridge_id.as_str())
            || self.work_pin.config_revision == 0
            || self.work_pin.config_revision > i64::MAX as u64
        {
            return Err(PartitionError::InvalidSource("message_pin"));
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for PartitionSourcePin {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            inbox_id: String,
            body_sha256: String,
            work_pin: BridgeWorkPin,
        }
        let wire = Wire::deserialize(deserializer)?;
        let value = Self {
            inbox_id: wire.inbox_id,
            body_sha256: wire.body_sha256,
            work_pin: wire.work_pin,
        };
        value
            .validate()
            .map_err(|_| serde::de::Error::custom("invalid partition source pin"))?;
        Ok(value)
    }
}

/// Checked message facts. Only a trusted capability observation can construct
/// this type; it cannot be deserialized from a callback, queue or API request.
/// The observation is historical message evidence, not live effect authority.
#[derive(Clone)]
pub struct NormalizedPartitionSource {
    pin: PartitionSourcePin,
    owner: ResourceOwner,
    config: BridgeConfig,
    pub(super) observation: PlatformCapabilityObservation,
    pub(super) upstream_admitted: bool,
}

impl NormalizedPartitionSource {
    /// The adapter must verify capability facts while the callback signature
    /// is checked, or recover them from equally verified durable evidence.
    /// StoredInbox alone does not prove sender kind, addressing, or loop facts.
    pub fn from_observation(
        inbox: &StoredInbox,
        bridge: &BridgeRecord,
        observation: PlatformCapabilityObservation,
    ) -> Result<Self, PartitionError> {
        let pin = PartitionSourcePin::for_inbox(inbox)?;
        let config = &bridge
            .pinned(&inbox.pin)
            .map_err(|_| PartitionError::SourceMismatch)?
            .config;
        let envelope = &inbox.envelope;
        if observation.source != pin
            || inbox.owner != bridge.owner
            || inbox.bot_id != config.connector.bot_id
            || inbox.key_id != config.connector.key_id
            || inbox.attestation.issuer != config.connector.issuer
            || !config.connector.route_ids.contains(&envelope.route_id)
            || platform(&envelope.platform)? != config.connector.platform
            || observation.platform != config.connector.platform
            || observation.chat_id != envelope.chat_id
            || observation.sender_id != envelope.sender_id
            || observation.conversation
                != ChannelConversationKind::from_wire(&envelope.conversation_type)
        {
            return Err(PartitionError::SourceMismatch);
        }
        if !opaque_id(&envelope.chat_id)
            || !opaque_id(&envelope.sender_id)
            || !opaque_id(&inbox.message_id)
            || crate::bridge::provider_timestamp(&envelope.timestamp).is_none()
            || inbox.accepted_at < 0
            || inbox.expires_at <= inbox.accepted_at
            || observation
                .provider_message_id
                .as_ref()
                .is_some_and(|id| !opaque_id(id))
        {
            return Err(PartitionError::InvalidSource("normalized_identity"));
        }
        if let Some(evidence) = &observation.reply_receipt {
            evidence.validate()?;
            if observation.replied_to_bot != VerifiedFlag::Yes {
                return Err(PartitionError::InvalidSource("reply_addressing_evidence"));
            }
        }
        if let ThreadEvidence::Verified(reference) = &observation.thread {
            reference.require_platform(&observation.platform)?;
        }
        Ok(Self {
            pin,
            owner: inbox.owner.clone(),
            config: config.clone(),
            observation,
            upstream_admitted: inbox.admission == InboxAdmission::Accepted,
        })
    }

    pub fn source_pin(&self) -> &PartitionSourcePin {
        &self.pin
    }
    pub fn bridge_id(&self) -> &BridgeId {
        &self.pin.work_pin.bridge_id
    }
    pub fn owner(&self) -> &ResourceOwner {
        &self.owner
    }
    pub fn issuer(&self) -> &str {
        &self.config.connector.issuer
    }
    pub fn bot_id(&self) -> &str {
        &self.config.connector.bot_id
    }
    pub fn platform(&self) -> &BridgePlatform {
        &self.observation.platform
    }
    pub fn chat_id(&self) -> &str {
        &self.observation.chat_id
    }
    pub fn sender_id(&self) -> &str {
        &self.observation.sender_id
    }
    pub fn provider_message_id(&self) -> Option<&str> {
        self.observation.provider_message_id.as_deref()
    }
    pub fn conversation_kind(&self) -> ChannelConversationKind {
        self.observation.conversation
    }
    /// Verified stable platform thread facts only. Protected generic callback
    /// context is never exposed as thread identity or routing authority.
    pub fn thread(&self) -> &ThreadEvidence {
        &self.observation.thread
    }
    pub(super) fn require_config(&self, config: &BridgeConfig) -> Result<(), PartitionError> {
        if &self.config != config {
            return Err(PartitionError::SourceMismatch);
        }
        config
            .validate_partition_admission()
            .map_err(|_| PartitionError::InvalidPolicy("partition_configuration"))
    }
}

fn platform(value: &str) -> Result<BridgePlatform, PartitionError> {
    match value {
        "telegram" => Ok(BridgePlatform::Telegram),
        "telegram-new" => Ok(BridgePlatform::TelegramNew),
        "discord" => Ok(BridgePlatform::Discord),
        "lark" => Ok(BridgePlatform::Lark),
        "feishu" => Ok(BridgePlatform::Feishu),
        "slack" => Ok(BridgePlatform::Slack),
        "whatsapp" => Ok(BridgePlatform::Whatsapp),
        "x" => Ok(BridgePlatform::X),
        _ => Err(PartitionError::InvalidSource("platform")),
    }
}
