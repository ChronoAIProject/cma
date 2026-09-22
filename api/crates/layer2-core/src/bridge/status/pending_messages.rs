//! Bounded previews of accepted channel messages before their durable turn starts.
use super::*;
use crate::{bridge::BridgeId, resource_access::CallerContext, AgentId};
use serde::Serialize;
use std::time::Duration;

pub const MAX_PENDING_PREVIEW_CHARS: usize = 1024;

#[derive(Debug, Clone, Serialize)]
pub struct PendingChannelMessage {
    pub command_id: crate::ids::AgentCommandId,
    pub response_id: crate::ids::AgentResponseId,
    pub agent_order: u64,
    pub admitted_at: String,
    pub platform: crate::bridge::BridgePlatform,
    pub text: String,
    pub text_truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PendingChannelMessages {
    Available {
        bridge_id: BridgeId,
        partition_id: String,
        generation: u64,
        agent_id: AgentId,
        through_sequence: u64,
        messages: Vec<PendingChannelMessage>,
        pending_truncated: bool,
        source_unavailable: bool,
        observed_at_ms: i64,
    },
    Unavailable,
    Redacted,
}

impl BridgePartitionService {
    pub async fn pending_messages(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        bridge: &BridgeId,
        id: &str,
    ) -> Result<PendingChannelMessages, PartitionStatusError> {
        tokio::time::timeout(
            Duration::from_secs(15),
            self.pending_messages_inner(context, credential, bridge, id),
        )
        .await
        .map_err(|_| PartitionStatusError::Unavailable)?
    }

    async fn pending_messages_inner(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        bridge: &BridgeId,
        id: &str,
    ) -> Result<PendingChannelMessages, PartitionStatusError> {
        if !service::allocation_id(id) {
            return Err(PartitionStatusError::Invalid);
        }
        let mut manager = self.management(context, credential, bridge).await?;
        let evidence = self
            .store
            .detail(&manager, id)
            .await?
            .ok_or(PartitionStatusError::NotFound)?;
        validation::allocation(&manager, &evidence.allocation)?;
        if evidence.allocation.id != id || evidence.observed_at_ms < 0 {
            return Err(PartitionStatusError::Corrupt);
        }
        let access = self
            .authorize_targets(context, credential, &mut manager, &evidence)
            .await?;
        let agent = access
            .checked
            .iter()
            .find_map(|(_, _, access)| access.snapshot.lineage.agent.as_ref());
        let result = if access.redacted {
            PendingChannelMessages::Redacted
        } else if let Some(agent) = agent.filter(|_| evidence.current) {
            let records = self.store.activity(&manager, &evidence.allocation).await?;
            let PartitionActivityView::Available {
                pending,
                pending_truncated,
                source_unavailable,
                observed_at_ms,
                ..
            } = service::activity_view(manager.record(), &evidence.allocation, agent, &records)?
            else {
                return Err(PartitionStatusError::Corrupt);
            };
            let mut messages = Vec::new();
            for command in pending
                .into_iter()
                .filter(|command| command.turn_sequence.is_none())
            {
                let source = records
                    .pending
                    .iter()
                    .find(|item| item.command.receipt.envelope.command_id == command.command_id)
                    .and_then(|item| item.source.as_ref())
                    .ok_or(PartitionStatusError::Corrupt)?;
                // Show only retained user text, never the runtime delivery prefix,
                // attachment data, reply credentials, or the original envelope.
                let original = source.envelope.text.as_deref().unwrap_or("");
                let text: String = original.chars().take(MAX_PENDING_PREVIEW_CHARS).collect();
                let text_truncated = original.chars().nth(MAX_PENDING_PREVIEW_CHARS).is_some();
                messages.push(PendingChannelMessage {
                    command_id: command.command_id,
                    response_id: command.response_id,
                    agent_order: command.agent_order,
                    admitted_at: command.admitted_at,
                    platform: evidence.allocation.config.connector.platform.clone(),
                    text,
                    text_truncated,
                });
            }
            PendingChannelMessages::Available {
                bridge_id: bridge.clone(),
                partition_id: id.into(),
                generation: evidence.allocation.identity.generation,
                agent_id: agent.id.clone(),
                through_sequence: agent.conversation.last_seq,
                messages,
                pending_truncated,
                source_unavailable,
                observed_at_ms,
            }
        } else {
            PendingChannelMessages::Unavailable
        };
        self.revalidate_targets(context, credential, &manager, &access)
            .await?;
        self.unchanged(&manager, &evidence).await?;
        self.revalidate_management(context, credential, &manager)
            .await?;
        Ok(result)
    }
}
