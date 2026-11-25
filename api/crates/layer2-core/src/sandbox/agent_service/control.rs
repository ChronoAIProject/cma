use super::{control_error, require, AgentService, AgentServiceError};
use crate::{
    now_rfc3339,
    sandbox::{
        store::{AgentControlResult, AgentControlTarget, AgentDeleteRequest, AgentStopRequest},
        Agent, AgentStopReason,
    },
    AgentId, Caller,
};

impl AgentService {
    pub async fn stop(
        &self,
        caller: &Caller,
        id: &AgentId,
        reason: AgentStopReason,
    ) -> Result<Agent, AgentServiceError> {
        let scheduled = self.schedule_stop(caller, id, reason).await?;
        self.fence_stop(scheduled.into_agent()).await
    }

    async fn schedule_stop(
        &self,
        caller: &Caller,
        id: &AgentId,
        reason: AgentStopReason,
    ) -> Result<AgentControlResult, AgentServiceError> {
        require(caller)?;
        let agent = self.owned_agent(caller, id).await?;
        self.store
            .schedule_agent_stop(AgentStopRequest {
                target: AgentControlTarget::from(&agent),
                reason,
                requested_at: now_rfc3339(),
            })
            .await
            .map_err(control_error)
    }

    async fn fence_stop(&self, scheduled: Agent) -> Result<Agent, AgentServiceError> {
        self.runtimes
            .fence(&scheduled.id, scheduled.work.next_fence)
            .await
            .map_err(super::support::runtime_error)?;
        Ok(scheduled)
    }

    pub async fn delete(&self, caller: &Caller, id: &AgentId) -> Result<Agent, AgentServiceError> {
        self.schedule_delete(caller, id)
            .await
            .map(AgentControlResult::into_agent)
    }

    async fn schedule_delete(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<AgentControlResult, AgentServiceError> {
        require(caller)?;
        let agent = self.owned_agent(caller, id).await?;
        self.store
            .schedule_agent_delete(AgentDeleteRequest {
                target: AgentControlTarget::from(&agent),
                requested_at: now_rfc3339(),
            })
            .await
            .map_err(control_error)
    }
}

#[cfg(feature = "qualification-hooks")]
impl AgentService {
    pub async fn stop_with_checkpoint(
        &self,
        caller: &Caller,
        id: &AgentId,
        reason: AgentStopReason,
        context: crate::qualification::ControlContext,
        checkpoint: &dyn crate::qualification::ControlCheckpoint,
    ) -> Result<Agent, AgentServiceError> {
        let scheduled = self.schedule_stop(caller, id, reason).await?;
        checkpoint
            .after_schedule(context, (&scheduled).into())
            .await;
        self.fence_stop(scheduled.into_agent()).await
    }

    pub async fn delete_with_result(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<AgentControlResult, AgentServiceError> {
        self.schedule_delete(caller, id).await
    }
}
