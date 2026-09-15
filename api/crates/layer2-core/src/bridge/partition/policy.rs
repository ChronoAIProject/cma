use serde::{Deserialize, Serialize};

use crate::bridge::{BridgeConfig, BridgePartitionPolicy, BridgePlacementPolicy};

use super::{opaque_id, PartitionError};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum IdentityAdmission {
    #[default]
    Deny,
    Open,
    Allowlist {
        ids: Vec<String>,
    },
}

impl IdentityAdmission {
    pub fn validate(&self) -> Result<(), PartitionError> {
        if let Self::Allowlist { ids } = self {
            let unique: std::collections::BTreeSet<_> = ids.iter().collect();
            if ids.is_empty()
                || ids.len() > 512
                || unique.len() != ids.len()
                || ids.iter().any(|id| !opaque_id(id))
            {
                return Err(PartitionError::InvalidPolicy("allowlist"));
            }
        }
        Ok(())
    }

    pub(super) fn permits(&self, id: &str) -> bool {
        match self {
            Self::Deny => false,
            Self::Open => true,
            Self::Allowlist { ids } => ids.iter().any(|candidate| candidate == id),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupAdmission {
    #[default]
    Deny,
    All,
    Mention,
    ReplyToBot,
    MentionOrReplyToBot,
}

/// Explicit policy, never transport-supplied authority. Defaults refuse all
/// traffic until an owner deliberately selects sender/chat and group gates.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelAdmissionPolicy {
    pub senders: IdentityAdmission,
    pub chats: IdentityAdmission,
    pub groups: GroupAdmission,
}

impl ChannelAdmissionPolicy {
    pub fn validate(&self) -> Result<(), PartitionError> {
        self.senders.validate()?;
        self.chats.validate()
    }
}

pub(in crate::bridge) fn validate_placement(config: &BridgeConfig) -> Result<(), PartitionError> {
    match (&config.placement, config.partition) {
        (BridgePlacementPolicy::ExistingAgent { .. }, BridgePartitionPolicy::FixedSharedAgent)
        | (BridgePlacementPolicy::NewSandbox { .. }, BridgePartitionPolicy::Conversation)
        | (BridgePlacementPolicy::NewSandbox { .. }, BridgePartitionPolicy::ConversationSender)
        | (BridgePlacementPolicy::NewSandbox { .. }, BridgePartitionPolicy::Thread)
        | (BridgePlacementPolicy::NewSandbox { .. }, BridgePartitionPolicy::ThreadSender)
        | (BridgePlacementPolicy::NewSandbox { .. }, BridgePartitionPolicy::SenderAcrossChats) => {
            Ok(())
        }
        _ => Err(PartitionError::InvalidPolicy("shared_context_placement")),
    }
}
