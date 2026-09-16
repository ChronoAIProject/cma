//! Availability of the exact locally published runtime. Source authority and
//! retained command claims are checked independently before execution.
use std::sync::Arc;

use layer2_core::{
    bridge::channel_control::{ChannelControlCommand, ChannelControlError, ChannelControlProof},
    sandbox::{AgentRuntimeRegistry, AgentRuntimeTarget},
    RuntimeCapabilities, RuntimeCapability,
};

use super::ingress::controls::ChannelControlRuntimeEligibility;

pub(crate) struct LocalChannelControlRuntime {
    registry: Arc<dyn AgentRuntimeRegistry>,
}

impl LocalChannelControlRuntime {
    pub(crate) fn new(registry: Arc<dyn AgentRuntimeRegistry>) -> Self {
        Self { registry }
    }

    pub(crate) async fn capabilities(
        &self,
        target: &AgentRuntimeTarget,
        deadline: tokio::time::Instant,
    ) -> Option<RuntimeCapabilities> {
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        let connection = tokio::time::timeout_at(deadline, self.registry.resolve(target))
            .await
            .ok()?
            .ok()?;
        if tokio::time::Instant::now() >= deadline
            || !connection
                .capabilities
                .supports(RuntimeCapability::Conversation)
        {
            return None;
        }
        // Resolving a published connection does not send input, claim a command
        // or confer source authority. Execution resolves and fences it again.
        Some(connection.capabilities)
    }
}

#[async_trait::async_trait]
impl ChannelControlRuntimeEligibility for LocalChannelControlRuntime {
    async fn require_available(
        &self,
        proof: &ChannelControlProof,
    ) -> Result<(), ChannelControlError> {
        proof.require_current()?;
        let intent = proof.intent();
        let target = AgentRuntimeTarget {
            agent_id: intent.reference.binding().source.target().agent_id.clone(),
            conversation_id: intent.runtime_conversation_id.clone(),
            runtime_generation: intent.runtime_generation,
        };
        let deadline = proof
            .source_invoke()
            .deadline()
            .min(proof.write().inbox().deadline());
        let capabilities = self
            .capabilities(&target, deadline)
            .await
            .ok_or(ChannelControlError::Unavailable)?;
        proof.require_current()?;
        let operation = match intent.request.command {
            ChannelControlCommand::Cancel => RuntimeCapability::Interrupt,
            ChannelControlCommand::Steer { .. } => RuntimeCapability::Steer,
        };
        if !capabilities.supports(operation) {
            return Err(ChannelControlError::Unavailable);
        }
        Ok(())
    }
}
