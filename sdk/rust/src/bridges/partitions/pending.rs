use super::{Bridges, Error, Result, bridge, validation};
use crate::bridges::BridgePlatform;
use serde::Deserialize;
use std::collections::BTreeSet;

/// Original accepted user text before its durable Agent turn starts.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingChannelMessage {
    /// Permanent command identity.
    pub command_id: String,
    /// Original response identity.
    pub response_id: String,
    /// Strictly increasing command order within this Agent.
    pub agent_order: u64,
    /// Original admission timestamp.
    pub admitted_at: String,
    /// Source platform.
    pub platform: BridgePlatform,
    /// Original user text, bounded to 1,024 Unicode characters.
    pub text: String,
    /// Whether the original text exceeds the preview bound.
    pub text_truncated: bool,
}

/// Authorized previews, or an explicit refusal to reveal message content.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum PendingChannelMessages {
    /// Current management and target authority permit these previews.
    Available {
        /// Exact requested Bridge.
        bridge_id: String,
        /// Exact requested allocation.
        partition_id: String,
        /// Current partition generation.
        generation: u64,
        /// Exact retained Agent.
        agent_id: String,
        /// Last durable Agent event sequence at observation.
        through_sequence: u64,
        /// At most 64 unstarted messages, in Agent order.
        messages: Vec<PendingChannelMessage>,
        /// More pending work exists beyond the preview bound.
        pending_truncated: bool,
        /// Some retained source evidence could not be read.
        source_unavailable: bool,
        /// Observation time in Unix milliseconds.
        observed_at_ms: u64,
    },
    /// No current target is available.
    Unavailable {},
    /// Current authority does not permit revealing target content.
    Redacted {},
}

impl Bridges<'_> {
    /// Read bounded original text without admitting or retrying channel work.
    pub async fn pending_messages(
        &self,
        bridge_id: &str,
        allocation_id: &str,
    ) -> Result<PendingChannelMessages> {
        bridge::id(bridge_id)?;
        if !validation::allocation_id(allocation_id) {
            return Err(Error::validation("invalid allocation identity"));
        }
        let result: PendingChannelMessages = self
            .client
            .get(
                &[
                    "bridges",
                    bridge_id,
                    "partitions",
                    allocation_id,
                    "pending-messages",
                ],
                &[],
            )
            .await?;
        result.check(bridge_id, allocation_id)?;
        Ok(result)
    }
}

impl PendingChannelMessages {
    fn check(&self, requested_bridge: &str, requested_allocation: &str) -> Result<()> {
        let Self::Available {
            bridge_id,
            partition_id,
            generation,
            agent_id,
            through_sequence,
            messages,
            observed_at_ms,
            ..
        } = self
        else {
            return Ok(());
        };
        let mut commands = BTreeSet::new();
        let mut responses = BTreeSet::new();
        let mut order = 0;
        if bridge_id != requested_bridge
            || partition_id != requested_allocation
            || !(1..=256).contains(generation)
            || !bridge::valid_id(agent_id)
            || *through_sequence > i64::MAX as u64
            || *observed_at_ms > i64::MAX as u64
            || messages.len() > 64
        {
            return Err(Error::protocol("invalid pending channel scope or bounds"));
        }
        for message in messages {
            if !bridge::valid_id(&message.command_id)
                || !bridge::valid_id(&message.response_id)
                || !commands.insert(&message.command_id)
                || !responses.insert(&message.response_id)
                || message.agent_order <= order
                || message.agent_order > i64::MAX as u64
                || message.text.chars().count() > 1024
                || message.admitted_at.trim().is_empty()
                || message.admitted_at.len() > 64
                || message.admitted_at.chars().any(char::is_control)
            {
                return Err(Error::protocol("invalid bounded pending channel message"));
            }
            order = message.agent_order;
        }
        Ok(())
    }
}
