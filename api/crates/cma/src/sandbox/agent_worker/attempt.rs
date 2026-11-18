//! Bounded worker claim, ownership and continuation orchestration.

use super::*;

impl AgentWorker {
    /// Keep the startup/cleanup execution state machine behind a boxed boundary
    /// so callers do not inherit its large stack frame during fixture startup.
    pub fn run_one(
        &self,
        id: &AgentId,
        now: SystemTime,
    ) -> Pin<Box<dyn Future<Output = Result<bool, SandboxStoreError>> + Send + '_>> {
        let id = id.clone();
        Box::pin(async move { self.run_one_inner(&id, now).await })
    }

    async fn run_one_inner(
        &self,
        id: &AgentId,
        now: SystemTime,
    ) -> Result<bool, SandboxStoreError> {
        if let Some(coordinator) = &self.config.ownership {
            let Some(authority) = coordinator
                .acquire(id)
                .await
                .map_err(|_| SandboxStoreError::Conflict)?
            else {
                return Ok(false);
            };
            let result = authority
                .run(Box::pin(self.run_claimed(id, now)))
                .await
                .map_err(|_| SandboxStoreError::Conflict)?;
            if result.is_ok()
                && self
                    .dependencies
                    .store
                    .get_agent(id)
                    .await?
                    .is_none_or(|agent| {
                        !agent.status.is_live() && agent.work.step == AgentWorkStep::None
                    })
            {
                self.dependencies.registry.detach_owner(&authority.token);
                coordinator
                    .handoff(&authority, authority.revoke_transports())
                    .await
                    .map_err(|_| SandboxStoreError::Conflict)?;
            }
            return result;
        }
        Box::pin(self.run_claimed(id, now)).await
    }

    async fn run_claimed(&self, id: &AgentId, now: SystemTime) -> Result<bool, SandboxStoreError> {
        let started = tokio::time::Instant::now();
        let Some(agent) = self
            .dependencies
            .store
            .claim_agent_work(
                id,
                &self.config.holder,
                rfc3339_at(now),
                rfc3339_at(now + self.config.lease_duration),
            )
            .await?
        else {
            return Ok(false);
        };
        let claim_ms = started.elapsed().as_millis() as u64;
        let step = agent.work.step;
        let executing = tokio::time::Instant::now();
        let result = Box::pin(self.execute(agent, now + started.elapsed())).await;
        tracing::info!(
            agent_id = %id,
            ?step,
            claim_ms,
            execution_ms = executing.elapsed().as_millis() as u64,
            published = result.is_ok(),
            "Agent work attempt completed"
        );
        result?;
        Ok(true)
    }

    /// Follow bounded, committed progress immediately, keeping each durable claim.
    pub(crate) async fn run_scheduled(
        &self,
        id: &AgentId,
        now: SystemTime,
    ) -> Result<bool, SandboxStoreError> {
        let started = tokio::time::Instant::now();
        let progressed = self.run_one(id, now).await?;
        if !progressed {
            return Ok(false);
        }
        for _ in 0..6 {
            let Some(agent) = self.dependencies.store.get_agent(id).await? else {
                break;
            };
            let startup = matches!(
                agent.work.step,
                AgentWorkStep::AddWorktree
                    | AgentWorkStep::MaterializeHome
                    | AgentWorkStep::OpenConversation
                    | AgentWorkStep::PublishReady
                    | AgentWorkStep::PublishConnection
                    | AgentWorkStep::DispatchFirstMessage
            );
            let cleanup = self.config.cleanup_continuation_enabled
                && agent.work.attempts == 0
                && matches!(
                    agent.work.step,
                    AgentWorkStep::FenceInteraction
                        | AgentWorkStep::CloseProcess
                        | AgentWorkStep::PublishStopped
                        | AgentWorkStep::EnsureProcessClosed
                        | AgentWorkStep::RemoveHome
                        | AgentWorkStep::RemoveWorktree
                        | AgentWorkStep::PublishDeleted
                );
            // A published retry is not progress, even when execution exceeded its
            // deadline. Reclaim only committed successful cleanup transitions.
            if !(startup || cleanup) || !agent.work.is_due(&rfc3339_at(now + started.elapsed())) {
                break;
            }
            if !self.run_one(id, now + started.elapsed()).await? {
                break;
            }
        }
        Ok(true)
    }
}
