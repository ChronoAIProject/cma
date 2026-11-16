use super::{AdmittedAgentCommand, AgentCommand, AgentCommandEnvelope, AgentCommandService};
use crate::AgentId;

/// Advisory post-commit scheduling only. Implementations must neither acquire
/// execution authority nor perform command work. Polling recovers lost hints.
#[async_trait::async_trait]
pub trait CommandDiscoveryWake: Send + Sync {
    async fn committed_cancel(&self, agent_id: &AgentId);
}

impl AgentCommandService {
    pub(super) async fn wake_committed_cancel(
        &self,
        proposed: &AgentCommandEnvelope,
        record: &AdmittedAgentCommand,
    ) {
        // Targeted reservations return the original record on replay. Only this
        // attempt's generated ID proves a new admission, including a confirmed
        // retry of its own commit. No notification follows an uncertain error.
        let committed = &record.receipt.envelope;
        if proposed.command_id == committed.command_id
            && proposed.agent_id == committed.agent_id
            && matches!(committed.command, AgentCommand::Interrupt)
        {
            if let Some(wake) = &self.discovery_wake {
                wake.committed_cancel(&committed.agent_id).await;
            }
        }
    }
}
