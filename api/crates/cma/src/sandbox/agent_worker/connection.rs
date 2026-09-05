use std::time::SystemTime;

use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{
    Agent, AgentStatus, AgentWorkStep, FirstMessageDelivery, SandboxStatus, WorkspaceStatus,
};

use super::startup_support::{live_target, registry_failure};
use super::support::{claimed_fence, AgentContext, WorkerFailure};
use super::AgentWorker;

const MAX_PUBLICATION_RETRIES: usize = 8;

impl AgentWorker {
    pub(super) async fn publish_connection(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let context = match self.connection_context(&agent).await {
            Ok(context) => context,
            Err(failure) => return self.handle_failure(agent, failure, now).await,
        };
        let target = match live_target(&agent) {
            Ok(target) => target,
            Err(failure) => return self.handle_failure(agent, failure, now).await,
        };
        claimed_fence(&agent, &self.config.holder)?;
        let attachment = match self.reattach(&agent, &context, &target).await {
            Ok(attachment) => attachment,
            Err(failure) => return self.handle_failure(agent, failure, now).await,
        };
        if let Err(error) = self.dependencies.registry.publish_and_activate(&attachment) {
            self.dependencies.registry.rollback(&attachment);
            return self
                .handle_failure(agent, registry_failure(error), now)
                .await;
        }
        self.finish_connection_publication(agent, now).await
    }

    pub(super) async fn connection_context(
        &self,
        agent: &Agent,
    ) -> Result<AgentContext, WorkerFailure> {
        if !matches!(
            agent.status,
            AgentStatus::Ready | AgentStatus::Working | AgentStatus::WaitingApproval
        ) || agent.requested_stop_reason.is_some()
        {
            return Err(WorkerFailure::terminal(
                "agent_connection_state_invalid",
                "connection publication requires an attachable live agent without stop intent",
            ));
        }
        let context = self.context(agent).await?;
        if context.sandbox.status != SandboxStatus::Active
            || context.workspace.status != WorkspaceStatus::Ready
            || context.worktree.status != layer2_core::sandbox::WorktreeStatus::Ready
        {
            return Err(WorkerFailure::retryable(
                "agent_parent_not_ready",
                "the sandbox, workspace, or worktree is not ready for connection publication",
            ));
        }
        Ok(context)
    }

    async fn finish_connection_publication(
        &self,
        mut agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let fence = claimed_fence(&agent, &self.config.holder)?;
        for _ in 0..MAX_PUBLICATION_RETRIES {
            let next = if agent
                .conversation
                .first_message
                .as_ref()
                .is_some_and(|message| message.state == FirstMessageDelivery::Queued)
            {
                AgentWorkStep::DispatchFirstMessage
            } else {
                AgentWorkStep::None
            };
            match self
                .progress(agent.clone(), next, now, None, Vec::new(), None, None)
                .await
            {
                Ok(()) => return Ok(()),
                Err(SandboxStoreError::Conflict) => {
                    let Some(current) = self.dependencies.store.get_agent(&agent.id).await? else {
                        return Err(SandboxStoreError::NotFound);
                    };
                    if !matches!(
                        current.status,
                        AgentStatus::Ready | AgentStatus::Working | AgentStatus::WaitingApproval
                    ) || current.work.step != AgentWorkStep::PublishConnection
                        || !current.work.owns(&self.config.holder, fence)
                        || live_target(&current).ok() != live_target(&agent).ok()
                    {
                        return Err(SandboxStoreError::Conflict);
                    }
                    agent = current;
                }
                Err(error) => return Err(error),
            }
        }
        Err(SandboxStoreError::Unavailable(
            "agent changed during every connection publication commit".into(),
        ))
    }
}
