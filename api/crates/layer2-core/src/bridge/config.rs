use serde::{Deserialize, Serialize};

use crate::profiles::{AgentProfileRef, SandboxProfileRef, WorkspaceProfileRef};
use crate::{AgentId, SandboxId, WorkspaceId, WorktreeId};

use super::BridgeError;

mod reuse;
pub use reuse::{BridgeSharedCapacity, BridgeSharedTrust};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgePlatform {
    Telegram,
    #[serde(rename = "telegram-new")]
    TelegramNew,
    Discord,
    Lark,
    Feishu,
}

/// Opaque custody locator, never the selected agent key value. Only the custody
/// adapter resolves it; it is deliberately absent from public views and Debug.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BridgeSecretRef(String);

impl std::fmt::Debug for BridgeSecretRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BridgeSecretRef([redacted])")
    }
}

impl BridgeSecretRef {
    pub fn generate() -> Self {
        Self(format!("brsec_{}", uuid::Uuid::new_v4().simple()))
    }

    pub fn new(reference: String) -> Result<Self, BridgeError> {
        if !Self::valid(&reference) {
            return Err(BridgeError::InvalidConfiguration("secret_reference"));
        }
        Ok(Self(reference))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn valid(reference: &str) -> bool {
        reference.strip_prefix("brsec_").is_some_and(|id| {
            id.len() == 32
                && id
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeConnector {
    pub issuer: String,
    pub base_url: String,
    pub bot_id: String,
    pub platform: BridgePlatform,
    pub route_ids: Vec<String>,
    pub key_id: String,
    pub custody: BridgeSecretRef,
    pub callback_url: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgePartitionPolicy {
    Conversation,
    #[default]
    ConversationSender,
    Thread,
    ThreadSender,
    FixedSharedAgent,
    SenderAcrossChats,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeAgentTarget {
    pub sandbox_id: SandboxId,
    pub workspace_id: WorkspaceId,
    pub worktree_id: WorktreeId,
    pub agent_id: AgentId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BridgePlacementPolicy {
    WorkspacePerPartition {
        sandbox_id: SandboxId,
        workspace_profile: WorkspaceProfileRef,
        agent_profile: AgentProfileRef,
        shared_trust: BridgeSharedTrust,
        capacity: BridgeSharedCapacity,
    },
    WorktreeAgentPerPartition {
        sandbox_id: SandboxId,
        workspace_id: WorkspaceId,
        agent_profile: AgentProfileRef,
        shared_trust: BridgeSharedTrust,
        capacity: BridgeSharedCapacity,
    },
    ExistingAgent {
        target: BridgeAgentTarget,
    },
    NewSandbox {
        sandbox_profile: SandboxProfileRef,
        workspace_profile: WorkspaceProfileRef,
        agent_profile: AgentProfileRef,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BridgeAdmissionPolicy {
    All,
    Allowlist {
        sender_ids: Vec<String>,
    },
    Scoped {
        policy: super::partition::ChannelAdmissionPolicy,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeReplyPolicy {
    /// Legacy wire spelling retained for rollback. Delivery requires explicit
    /// reply tool calls by a channel-created Agent, including during its turn.
    /// This policy never creates an automatic final-response producer.
    FinalResponse,
    /// Reserved until verified approval controls are available. It does not
    /// authorize automatic final-answer delivery.
    FinalResponseAndApprovals,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeLimits {
    pub max_partitions: u32,
    pub max_sandboxes: u32,
    pub max_queued_messages: u32,
    pub max_messages_per_minute: u32,
    pub max_payload_bytes: u32,
    pub inbox_retention_seconds: u32,
    pub reply_secret_retention_seconds: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeConfig {
    pub display_name: String,
    pub connector: BridgeConnector,
    pub partition: BridgePartitionPolicy,
    pub placement: BridgePlacementPolicy,
    pub admission: BridgeAdmissionPolicy,
    pub reply: BridgeReplyPolicy,
    pub limits: BridgeLimits,
}

impl BridgeConfig {
    pub fn validate(&self) -> Result<(), BridgeError> {
        let bad = BridgeError::InvalidConfiguration;
        // Together with the 256-generation bound this keeps complete binding
        // history below 5 MiB, including BSON overhead, in one atomic record.
        if serde_json::to_vec(self)
            .map_err(|_| bad("configuration_size"))?
            .len()
            > 16_384
        {
            return Err(bad("configuration_size"));
        }
        if self.display_name.trim().is_empty()
            || self.display_name.len() > 200
            || self.display_name.chars().any(char::is_control)
        {
            return Err(bad("display_name"));
        }
        let c = &self.connector;
        if !valid_id(&c.bot_id)
            || !valid_id(&c.key_id)
            || !BridgeSecretRef::valid(c.custody.as_str())
        {
            return Err(bad("binding_identity"));
        }
        for (endpoint, callback) in [
            (&c.issuer, false),
            (&c.base_url, false),
            (&c.callback_url, true),
        ] {
            let url = url::Url::parse(endpoint).map_err(|_| bad("connector_url"))?;
            // Issuer/base are checked against deployment configuration by the
            // authenticated service/adapter; existing internal providers use
            // HTTP. The platform-facing callback always requires HTTPS.
            if !(url.scheme() == "https" || (!callback && url.scheme() == "http"))
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(bad("connector_url"));
            }
        }
        let routes: std::collections::BTreeSet<_> = c.route_ids.iter().collect();
        if routes.is_empty()
            || routes.len() > 64
            || routes.len() != c.route_ids.len()
            || routes.iter().any(|id| !valid_id(id))
        {
            return Err(bad("route_ids"));
        }
        self.placement.validate_reuse()?;
        match &self.placement {
            BridgePlacementPolicy::WorkspacePerPartition { .. }
            | BridgePlacementPolicy::WorktreeAgentPerPartition { .. } => {}
            BridgePlacementPolicy::ExistingAgent { target } => {
                if [
                    target.sandbox_id.as_str(),
                    target.workspace_id.as_str(),
                    target.worktree_id.as_str(),
                    target.agent_id.as_str(),
                ]
                .iter()
                .any(|id| !valid_id(id))
                {
                    return Err(bad("target"));
                }
            }
            BridgePlacementPolicy::NewSandbox {
                sandbox_profile,
                workspace_profile,
                agent_profile,
            } => {
                if sandbox_profile.revision == 0
                    || workspace_profile.revision == 0
                    || agent_profile.revision == 0
                    || !valid_id(sandbox_profile.id.as_str())
                    || !valid_id(workspace_profile.id.as_str())
                    || !valid_id(agent_profile.id.as_str())
                {
                    return Err(bad("profile_pins"));
                }
            }
        }
        if let BridgeAdmissionPolicy::Allowlist { sender_ids } = &self.admission {
            let unique: std::collections::BTreeSet<_> = sender_ids.iter().collect();
            if unique.is_empty()
                || unique.len() > 512
                || unique.len() != sender_ids.len()
                || unique.iter().any(|id| !valid_id(id))
            {
                return Err(bad("sender_ids"));
            }
        }
        if let BridgeAdmissionPolicy::Scoped { policy } = &self.admission {
            policy.validate().map_err(|_| bad("admission_policy"))?;
        }
        let l = &self.limits;
        if l.max_partitions == 0
            || l.max_partitions > 100_000
            || l.max_sandboxes == 0
            || l.max_sandboxes > l.max_partitions
            || l.max_queued_messages == 0
            || l.max_queued_messages > 100_000
            || l.max_messages_per_minute == 0
            || l.max_messages_per_minute > 100_000
            || l.max_payload_bytes == 0
            || l.max_payload_bytes > 1_048_576
            || l.inbox_retention_seconds == 0
            || l.inbox_retention_seconds > 2_592_000
            || l.reply_secret_retention_seconds == 0
            || l.reply_secret_retention_seconds > l.inbox_retention_seconds
        {
            return Err(bad("limits"));
        }
        Ok(())
    }

    /// Fresh setup/enable and partition admission require explicit context
    /// sharing. Historical generation reads retain their original syntax.
    pub fn validate_partition_admission(&self) -> Result<(), BridgeError> {
        self.validate()?;
        super::partition::validate_placement(self)
            .map_err(|_| BridgeError::InvalidConfiguration("partition_context"))
    }
}

pub(super) fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.:/".contains(&b))
}
