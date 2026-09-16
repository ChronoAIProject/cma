use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::bridge::BridgePartitionPolicy;

use super::{NormalizedPartitionSource, PartitionError};

/// Bounded durable identity. Deserializing this value grants no authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NormalizedPartitionKey {
    version: u16,
    hash: String,
}

impl NormalizedPartitionKey {
    pub fn version(&self) -> u16 {
        self.version
    }
    pub fn hash(&self) -> &str {
        &self.hash
    }

    pub(super) fn for_source(
        source: &NormalizedPartitionSource,
        policy: BridgePartitionPolicy,
        thread: Option<&str>,
    ) -> Result<Self, PartitionError> {
        let (chat, conversation, sender) = match policy {
            BridgePartitionPolicy::FixedSharedAgent => (None, None, None),
            BridgePartitionPolicy::SenderAcrossChats => (None, None, Some(source.sender_id())),
            BridgePartitionPolicy::Conversation | BridgePartitionPolicy::Thread => (
                Some(source.chat_id()),
                Some(source.conversation_kind()),
                None,
            ),
            BridgePartitionPolicy::ConversationSender | BridgePartitionPolicy::ThreadSender => (
                Some(source.chat_id()),
                Some(source.conversation_kind()),
                Some(source.sender_id()),
            ),
        };
        // A tuple gives each opaque identity an unambiguous boundary. Policy
        // and version separate namespaces; route/message/display/config-change
        // metadata and protected transport context never enter this encoding.
        let encoded = serde_json::to_vec(&(
            "cma.bridge.partition",
            1_u16,
            source.issuer(),
            &source.source_pin().work_pin().bridge_id,
            source.bot_id(),
            source.platform(),
            policy,
            chat,
            conversation,
            sender,
            thread,
        ))
        .map_err(|_| PartitionError::InvalidKey)?;
        Ok(Self {
            version: 1,
            hash: format!("{:x}", Sha256::digest(encoded)),
        })
    }
}

impl<'de> Deserialize<'de> for NormalizedPartitionKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            version: u16,
            hash: String,
        }
        let wire = Wire::deserialize(deserializer)?;
        if wire.version != 1 || !super::digest(&wire.hash) {
            return Err(serde::de::Error::custom("invalid normalized partition key"));
        }
        Ok(Self {
            version: wire.version,
            hash: wire.hash,
        })
    }
}

/// Publicly safe description of who intentionally shares model context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartitionContextSharing {
    ChatSender,
    Chat,
    ThreadSender,
    Thread,
    SenderAcrossChats,
    FixedAgent,
}

impl From<BridgePartitionPolicy> for PartitionContextSharing {
    fn from(value: BridgePartitionPolicy) -> Self {
        match value {
            BridgePartitionPolicy::ConversationSender => Self::ChatSender,
            BridgePartitionPolicy::Conversation => Self::Chat,
            BridgePartitionPolicy::ThreadSender => Self::ThreadSender,
            BridgePartitionPolicy::Thread => Self::Thread,
            BridgePartitionPolicy::SenderAcrossChats => Self::SenderAcrossChats,
            BridgePartitionPolicy::FixedSharedAgent => Self::FixedAgent,
        }
    }
}

/// Sender isolation never changes the audience of a group/channel reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyVisibility {
    OriginalPrivateChat,
    OriginalGroupOrChannel,
}
