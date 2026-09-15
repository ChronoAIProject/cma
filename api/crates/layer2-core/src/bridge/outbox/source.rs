use serde::{Deserialize, Serialize};

use crate::bridge::{
    allocation::{AllocationPlacement, AllocationRecord, AllocationState},
    ingress::{InboxAdmission, StoredInbox},
    partition::{NormalizedPartitionSource, PartitionAdmissionDecision, ThreadEvidence},
    AcceptedWorkPolicy, BridgeAgentTarget, BridgeConfig, BridgeRecord, BridgeReplyPolicy,
    BridgeState, BridgeWorkPin,
};
use crate::ids::{AgentCommandId, AgentResponseId};
use crate::resource_access::{ResourceActor, ResourceOwner};
use crate::sandbox::agent_io::{
    AdmissionReceipt, AgentCommand, AgentSourceKind, ChannelCommandSidecar,
};

use super::OutboxError;

/// Protected historical routing data; intentionally no Debug. This is not a
/// delivery permission. Storage must verify the command's Channel sidecar and
/// the exact retained inbox/allocation/command rows in its enqueue transaction.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboxSource {
    pub(super) inbox_id: String,
    pub(super) inbox_sha256: String,
    pub(super) allocation_id: String,
    pub(super) command_id: AgentCommandId,
    pub(super) response_id: AgentResponseId,
    pub(super) admission_sha256: String,
    pub(super) owner: ResourceOwner,
    pub(super) pin: BridgeWorkPin,
    pub(super) config: BridgeConfig,
    pub(super) target: BridgeAgentTarget,
    pub(super) message_id: String,
    pub(super) chat_id: String,
    pub(super) source_sha256: String,
    pub(super) reply_expires_at_ms: Option<i64>,
    pub(super) route_expires_at_ms: Option<i64>,
}

impl OutboxSource {
    pub fn capture(
        bridge: &BridgeRecord,
        inbox: &StoredInbox,
        allocation: &AllocationRecord,
        receipt: &AdmissionReceipt,
        sidecar: &ChannelCommandSidecar,
    ) -> Result<Self, OutboxError> {
        allocation
            .validate()
            .map_err(|_| OutboxError::SourceMismatch)?;
        receipt
            .validate_integrity()
            .map_err(|_| OutboxError::SourceMismatch)?;
        let normalized = NormalizedPartitionSource::from_stored(inbox, bridge)
            .map_err(|_| OutboxError::SourceMismatch)?;
        let config = &bridge
            .pinned(&inbox.pin)
            .map_err(|_| OutboxError::SourceMismatch)?
            .config;
        let envelope = &receipt.envelope;
        let actor_matches = matches!(&envelope.actor, ResourceActor::Channel {
            bridge_id, platform, chat_id, sender_id, grant_id
        } if bridge_id == inbox.pin.bridge_id.as_str() && platform == &inbox.envelope.platform
            && chat_id == normalized.chat_id() && sender_id == normalized.sender_id()
            && match &allocation.placement {
                AllocationPlacement::ExistingAgent { grant_id: original, .. } => grant_id == original,
                AllocationPlacement::NewSandbox { .. } | AllocationPlacement::ReuseParent { .. } => true,
            });
        let AgentCommand::Message { metadata, .. } = &envelope.command else {
            return Err(OutboxError::SourceMismatch);
        };
        if inbox.admission != InboxAdmission::Accepted
            || allocation.state != AllocationState::Ready
            || !allocation_matches(bridge, allocation, &inbox.pin, config)
            || allocation.owner != inbox.owner
            || envelope.owner != inbox.owner
            || envelope.agent_id != allocation.placement.target().agent_id
            || envelope.source.kind != AgentSourceKind::Channel
            || envelope.source.reference != inbox.id
            || envelope.correlation_id != inbox.id
            || !actor_matches
            || metadata.get("channel_inbox_id") != Some(&inbox.id)
            || metadata.get("channel_allocation_id") != Some(&allocation.id)
            || metadata.get("channel_body_sha256") != Some(&inbox.attestation.body_sha256)
            || metadata.get("channel_message_id") != Some(&inbox.message_id)
            || metadata.get("channel_config_revision")
                != Some(&inbox.pin.config_revision.to_string())
            || config.reply == BridgeReplyPolicy::Disabled
            || sidecar.schema_version != 1
            || sidecar.inbox_id != inbox.id
            || sidecar.allocation_id != allocation.id
            || sidecar.body_sha256 != inbox.attestation.body_sha256
            || sidecar.command_id != envelope.command_id
            || Some(&sidecar.response_id) != receipt.response_id.as_ref()
            || sidecar.request_fingerprint != envelope.request_fingerprint
        {
            return Err(OutboxError::SourceMismatch);
        }
        match normalized
            .admit(config)
            .map_err(|_| OutboxError::SourceMismatch)?
        {
            PartitionAdmissionDecision::Admitted { key, .. } if key == allocation.identity.key => {}
            _ => return Err(OutboxError::SourceMismatch),
        }
        // Discord interaction routing expires independently of key/token
        // validity. Refuse after its original window rather than accept NyxID's
        // fallback to a regular channel audience. Keep a transport margin.
        let route_expires_at_ms = if inbox.envelope.platform == "discord"
            && inbox.envelope.protected_thread_context_present
            && !matches!(normalized.thread(), ThreadEvidence::Verified(_))
        {
            Some(
                crate::seconds_from_rfc3339(&inbox.envelope.timestamp)
                    .and_then(|seconds| i64::try_from(seconds).ok())
                    .and_then(|seconds| seconds.checked_add(14 * 60))
                    .and_then(|seconds| seconds.checked_mul(1000))
                    .ok_or(OutboxError::Corrupt)?,
            )
        } else {
            None
        };
        let result = Self {
            inbox_id: inbox.id.clone(),
            inbox_sha256: inbox.attestation.body_sha256.clone(),
            allocation_id: allocation.id.clone(),
            command_id: envelope.command_id.clone(),
            response_id: receipt
                .response_id
                .clone()
                .ok_or(OutboxError::SourceMismatch)?,
            admission_sha256: super::hash(receipt)?,
            owner: inbox.owner.clone(),
            pin: inbox.pin.clone(),
            config: config.clone(),
            target: allocation.placement.target().clone(),
            message_id: inbox.message_id.clone(),
            chat_id: normalized.chat_id().into(),
            source_sha256: super::hash(&(
                &inbox.attestation,
                &inbox.capabilities,
                &inbox.bot_id,
                &inbox.key_id,
                &inbox.message_id,
                &inbox.envelope,
                &allocation.identity,
                allocation.placement.target(),
            ))?,
            reply_expires_at_ms: inbox
                .reply_expires_at
                .map(|s| s.checked_mul(1000).ok_or(OutboxError::Corrupt))
                .transpose()?,
            route_expires_at_ms,
        };
        result.validate()?;
        Ok(result)
    }
    pub fn inbox_id(&self) -> &str {
        &self.inbox_id
    }
    pub fn pin(&self) -> &BridgeWorkPin {
        &self.pin
    }
    pub fn command_id(&self) -> &AgentCommandId {
        &self.command_id
    }
    pub fn response_id(&self) -> &AgentResponseId {
        &self.response_id
    }
    pub fn config(&self) -> &BridgeConfig {
        &self.config
    }
    pub fn target(&self) -> &BridgeAgentTarget {
        &self.target
    }
    pub fn owner(&self) -> &ResourceOwner {
        &self.owner
    }
    pub fn allocation_id(&self) -> &str {
        &self.allocation_id
    }
    pub fn chat_id(&self) -> &str {
        &self.chat_id
    }
    pub fn message_id(&self) -> &str {
        &self.message_id
    }
    pub fn fingerprint(&self) -> Result<String, OutboxError> {
        super::hash(self)
    }
    pub fn require_bridge(&self, bridge: &BridgeRecord) -> Result<(), OutboxError> {
        self.validate()?;
        if bridge.owner != self.owner
            || bridge
                .pinned(&self.pin)
                .map_err(|_| OutboxError::SourceMismatch)?
                .config
                != self.config
        {
            return Err(OutboxError::SourceMismatch);
        }
        if bridge.emergency.outbound_stopped
            || matches!(
                bridge.state,
                BridgeState::Disabled {
                    accepted_work: AcceptedWorkPolicy::SuspendOriginal
                } | BridgeState::Retired {
                    accepted_work: AcceptedWorkPolicy::SuspendOriginal
                }
            )
            || bridge.generations.iter().any(|g| {
                g.config_revision > self.pin.config_revision
                    && g.previous_work == Some(AcceptedWorkPolicy::SuspendOriginal)
            })
        {
            return Err(OutboxError::Suspended);
        }
        Ok(())
    }
    pub(super) fn validate(&self) -> Result<(), OutboxError> {
        if ![
            &self.inbox_id,
            &self.inbox_sha256,
            &self.admission_sha256,
            &self.source_sha256,
        ]
        .iter()
        .all(|s| super::digest(s))
            || ![
                self.command_id.as_str(),
                self.response_id.as_str(),
                &self.allocation_id,
                &self.message_id,
                &self.chat_id,
                self.pin.bridge_id.as_str(),
            ]
            .iter()
            .all(|s| super::id(s))
            || self.pin.config_revision == 0
            || self.pin.config_revision > 256
            || self.reply_expires_at_ms.is_some_and(|n| n <= 0)
            || self.route_expires_at_ms.is_some_and(|n| n <= 0)
            || self.config.reply == BridgeReplyPolicy::Disabled
        {
            return Err(OutboxError::Corrupt);
        }
        self.config.validate().map_err(|_| OutboxError::Corrupt)
    }
}

fn allocation_matches(
    bridge: &BridgeRecord,
    allocation: &AllocationRecord,
    inbound: &BridgeWorkPin,
    config: &BridgeConfig,
) -> bool {
    match &allocation.placement {
        AllocationPlacement::ExistingAgent { .. } => {
            allocation.pin == *inbound && allocation.config == *config
        }
        AllocationPlacement::NewSandbox { .. } | AllocationPlacement::ReuseParent { .. } => {
            let original = &allocation.config;
            allocation.pin.bridge_id == inbound.bridge_id
                && bridge
                    .pinned(&allocation.pin)
                    .is_ok_and(|generation| generation.config == *original)
                && config.placement.creates_partition_agent()
                && original.connector.issuer == config.connector.issuer
                && original.connector.bot_id == config.connector.bot_id
                && original.connector.platform == config.connector.platform
                && original.partition == config.partition
        }
    }
}
