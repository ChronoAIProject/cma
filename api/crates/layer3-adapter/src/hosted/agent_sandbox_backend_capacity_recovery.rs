use super::{AgentSandboxBackend, ConversationChannel, ConversationUpdate};
use tokio::sync::mpsc;

#[async_trait::async_trait]
impl crate::hosted::CapacityRecoveryNativeTransport for AgentSandboxBackend {
    async fn reattach_existing(
        &self,
        request: &crate::agent_capacity_recovery::CapacityRecoveryRequest,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> crate::agent_capacity_recovery::CapacityRecoveryResult<(
        ConversationChannel,
        crate::agent_capacity_recovery::CapacityRecoveryConfirmation,
    )> {
        self.transport
            .reattach_existing_capacity_recovery(request, updates)
            .await
    }
}
