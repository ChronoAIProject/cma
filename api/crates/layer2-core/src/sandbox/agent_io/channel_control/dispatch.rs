use super::ChannelControlExecutionAuthorization;
use crate::sandbox::{AgentOperationDelivery, AgentRuntimeTarget};
use layer3_adapter::{AdapterError, ConversationInputGuard};

pub(in crate::sandbox) struct ChannelControlDispatchGuard {
    pub authority: ChannelControlExecutionAuthorization,
    pub target: AgentRuntimeTarget,
    pub operation_sequence: u64,
    pub turn_sequence: u64,
}
#[async_trait::async_trait]
impl ConversationInputGuard for ChannelControlDispatchGuard {
    async fn check(&self) -> layer3_adapter::Result<()> {
        let refused = || {
            AdapterError::Rejected(
                "Channel control execution authority is no longer current".into(),
            )
        };
        let current = self.authority.refreshed().await.map_err(|_| refused())?;
        let fence = current.fence();
        let command = &fence.snapshot().admitted.command;
        fence.require_dispatch(command).map_err(|_| refused())?;
        let agent = fence
            .proof()
            .resources()
            .lineage
            .agent
            .as_ref()
            .ok_or_else(refused)?;
        let dispatch = command.work.dispatch.as_ref().ok_or_else(refused)?;
        if agent.id != self.target.agent_id
            || agent.conversation.conversation_id.as_ref() != Some(&self.target.conversation_id)
            || agent.conversation.runtime_generation != Some(self.target.runtime_generation)
            || agent.conversation.active_turn != Some(self.turn_sequence)
            || dispatch.operation_sequence != self.operation_sequence
            || dispatch.turn_sequence != self.turn_sequence
            || dispatch.runtime_generation != self.target.runtime_generation
            || dispatch.delivery != AgentOperationDelivery::DispatchFenced
            || Some(&dispatch.claim) != fence.claim()
            || agent
                .conversation
                .operation
                .as_ref()
                .is_none_or(|operation| {
                    operation.sequence != self.operation_sequence
                        || operation.turn_sequence != self.turn_sequence
                        || operation.runtime_generation != self.target.runtime_generation
                        || operation.state != AgentOperationDelivery::DispatchFenced
                })
        {
            return Err(refused());
        }
        fence.require_current().map_err(|_| refused())
    }
}
