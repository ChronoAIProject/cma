use super::*;
use crate::sandbox::{AgentLogProjectionState, BoundedAgentLogFold};

impl AgentReadService {
    pub(super) async fn legacy_state(
        &self,
        agent: &Agent,
        visit: &mut (dyn FnMut(AgentLogEntry) -> Result<(), SandboxStoreError> + Send),
    ) -> Result<AgentLogProjectionState, AgentReadError> {
        if agent.conversation.last_seq == 0 {
            return Err(corrupt("agent log has no creation record"));
        }
        if agent.conversation.last_seq > 10_128 {
            return Err(AgentReadError::PaginationRequired);
        }
        let mut fold = BoundedAgentLogFold::default();
        self.store
            .visit_agent_log_prefix(&agent.id, agent.conversation.last_seq, &mut |entry| {
                fold.observe(agent, &entry)
                    .map_err(SandboxStoreError::Corrupt)?;
                visit(entry)
            })
            .await
            .map_err(log_store_error)?;
        self.reject_uncommitted_tail(agent, agent.conversation.last_seq)
            .await?;
        fold.finish(agent).map_err(AgentReadError::Corrupt)
    }

    pub(super) async fn legacy_usage(
        &self,
        agent: &Agent,
    ) -> Result<AgentUsageReading, AgentReadError> {
        let state = self.legacy_state(agent, &mut |_| Ok(())).await?;
        Ok(AgentUsageReading {
            agent_id: agent.id.clone(),
            through_seq: state.through_seq,
            usage: state.usage(),
        })
    }
}
