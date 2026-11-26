//! Reconcile the exact Open on the runtime named by the admitted restart.
use std::time::{Duration, SystemTime};

use layer2_core::{
    rfc3339_at,
    sandbox::{
        store::{AgentRestartTeardownPublication, SandboxStoreError},
        Agent,
    },
    seconds_from_rfc3339,
};
use layer3_adapter::wire::AgentRestartFence;

use super::{
    support::{claimed_fence, WorkerFailure},
    AgentWorker,
};

impl AgentWorker {
    pub(super) async fn restart_teardown(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let started = tokio::time::Instant::now();
        let teardown = self
            .dependencies
            .store
            .agent_restart_teardown(&agent)
            .await?;
        let context = match self.cleanup_context(&agent).await {
            Ok(context) => context,
            Err(_) => {
                return self
                    .retry_restart_teardown(agent, now + started.elapsed())
                    .await
            }
        };
        let fence = claimed_fence(&agent, &self.config.holder)?;
        let expires = agent
            .work
            .lease
            .as_ref()
            .and_then(|lease| seconds_from_rfc3339(&lease.expires_at))
            .ok_or(SandboxStoreError::Conflict)?;
        let sent_at = seconds_from_rfc3339(&rfc3339_at(now + started.elapsed()))
            .ok_or(SandboxStoreError::Conflict)?;
        if sent_at >= expires {
            return Err(SandboxStoreError::Conflict);
        }
        let Some(native) = self.dependencies.adapter.runtime_recovery() else {
            return self
                .retry_restart_teardown(agent, now + started.elapsed())
                .await;
        };
        let prove = async {
            let mut request = teardown.fence(fence, context.worktree.path.clone(), false);
            if native
                .fence_agent_restart(&context.runtime, &request)
                .await
                .is_err()
            {
                request = teardown.fence(fence, context.worktree.path.clone(), true);
                native
                    .fence_agent_restart(&context.runtime, &request)
                    .await?;
            }
            Ok::<AgentRestartFence, layer3_adapter::AdapterError>(request)
        };
        let timeout = Duration::from_secs(expires - sent_at).min(self.config.open_timeout);
        let Ok(Ok(acknowledgement)) = tokio::time::timeout(timeout, prove).await else {
            return self
                .retry_restart_teardown(agent, now + started.elapsed())
                .await;
        };
        self.dependencies
            .registry
            .fence(&agent.id, agent.conversation.runtime_generation, fence);
        self.dependencies
            .store
            .publish_agent_restart_teardown(AgentRestartTeardownPublication {
                claimed: agent,
                teardown,
                acknowledgement,
                parents: context.publication_parents(),
                published_at: rfc3339_at(now + started.elapsed()),
            })
            .await
    }

    async fn retry_restart_teardown(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        self.handle_failure(
            agent,
            WorkerFailure::retryable(
                "agent_restart_teardown_pending",
                "the original runtime has yet to confirm cleanup of the admitted Agent restart",
            ),
            now,
        )
        .await
    }
}
