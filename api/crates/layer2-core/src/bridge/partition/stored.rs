use serde::{Deserialize, Serialize};

use crate::bridge::{ingress::StoredInbox, BridgePlatform, BridgeRecord};

use super::*;

/// Normalization contract, not a claim of deployed/live platform verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityContract {
    NyxidPlatformV1,
}

/// Historical nonsecret message evidence. This value has no execution authority;
/// only the authenticated callback write may initially persist it. Deserialized
/// evidence must match the exact retained inbox and pinned bridge configuration.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredPartitionObservation {
    version: u16,
    contract: CapabilityContract,
    source: PartitionSourcePin,
    message_id: String,
    provider_message_id: Option<String>,
    platform: BridgePlatform,
    chat_id: String,
    sender_id: String,
    conversation: ChannelConversationKind,
    sender_kind: ChannelSenderKind,
    event_kind: ChannelEventKind,
    loop_evidence: LoopEvidence,
    mentioned_bot: VerifiedFlag,
    replied_to_bot: VerifiedFlag,
    thread: StoredThreadEvidence,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum StoredThreadEvidence {
    Unknown,
    Unsupported,
    Missing,
    Verified {
        platform_kind: StableThreadKind,
        id: String,
    },
}

impl StoredPartitionObservation {
    pub fn contract(&self) -> CapabilityContract {
        self.contract
    }

    pub(in crate::bridge) fn from_observation(
        inbox: &StoredInbox,
        facts: &PlatformCapabilityObservation,
        contract: CapabilityContract,
    ) -> Self {
        Self {
            version: 1,
            contract,
            source: facts.source.clone(),
            message_id: inbox.message_id.clone(),
            provider_message_id: facts.provider_message_id.clone(),
            platform: facts.platform.clone(),
            chat_id: facts.chat_id.clone(),
            sender_id: facts.sender_id.clone(),
            conversation: facts.conversation,
            sender_kind: facts.sender_kind,
            event_kind: facts.event_kind,
            loop_evidence: facts.loop_evidence,
            mentioned_bot: facts.mentioned_bot,
            replied_to_bot: facts.replied_to_bot,
            thread: match &facts.thread {
                ThreadEvidence::Unknown => StoredThreadEvidence::Unknown,
                ThreadEvidence::Unsupported => StoredThreadEvidence::Unsupported,
                ThreadEvidence::Missing => StoredThreadEvidence::Missing,
                ThreadEvidence::Verified(reference) => StoredThreadEvidence::Verified {
                    platform_kind: reference.kind(),
                    id: reference.id().into(),
                },
            },
        }
    }

    fn observation(&self) -> Result<PlatformCapabilityObservation, PartitionError> {
        if self.version != 1 {
            return Err(PartitionError::InvalidSource("capability_version"));
        }
        Ok(PlatformCapabilityObservation {
            source: self.source.clone(),
            platform: self.platform.clone(),
            chat_id: self.chat_id.clone(),
            sender_id: self.sender_id.clone(),
            provider_message_id: self.provider_message_id.clone(),
            conversation: self.conversation,
            sender_kind: self.sender_kind,
            event_kind: self.event_kind,
            loop_evidence: self.loop_evidence,
            mentioned_bot: self.mentioned_bot,
            replied_to_bot: self.replied_to_bot,
            thread: match &self.thread {
                StoredThreadEvidence::Unknown => ThreadEvidence::Unknown,
                StoredThreadEvidence::Unsupported => ThreadEvidence::Unsupported,
                StoredThreadEvidence::Missing => ThreadEvidence::Missing,
                StoredThreadEvidence::Verified { platform_kind, id } => ThreadEvidence::Verified(
                    StableThreadReference::from_verified_field(*platform_kind, id.clone())?,
                ),
            },
        })
    }
}

impl NormalizedPartitionSource {
    /// A trusted inbox store must return this exact row. Its retained facts
    /// prove historical identity only; effects still need separate live proof.
    pub fn from_stored(inbox: &StoredInbox, bridge: &BridgeRecord) -> Result<Self, PartitionError> {
        let retained = inbox
            .capabilities
            .as_ref()
            .ok_or(PartitionError::InvalidSource(
                "capability_evidence_unavailable",
            ))?;
        if retained.message_id != inbox.message_id {
            return Err(PartitionError::SourceMismatch);
        }
        let facts = retained.observation()?;
        Self::from_observation(inbox, bridge, facts)
    }
}
