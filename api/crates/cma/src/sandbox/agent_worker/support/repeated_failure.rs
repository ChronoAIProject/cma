//! A point read selects the proposal; publication rechecks the exact marker.
use super::*;
use layer2_core::sandbox::store::plan_agent_repeated_failure;

impl AgentWorker {
    pub(super) async fn retry_repeated_failure_at(
        &self,
        agent: &Agent,
        failure: &WorkerFailure,
        now: SystemTime,
        ready_at: SystemTime,
    ) -> Result<bool, SandboxStoreError> {
        if !self.config.retry_single_marker_enabled || agent.conversation.last_seq == 0 {
            return Ok(false);
        }
        let started = tokio::time::Instant::now();
        let last = self
            .dependencies
            .store
            .agent_log_entry(&agent.id, agent.conversation.last_seq)
            .await?;
        let elapsed = started.elapsed();
        let Some(retry) = plan_agent_repeated_failure(
            agent,
            last.as_ref(),
            failure.class,
            &self.config.holder,
            claimed_fence(agent, &self.config.holder)?,
            &rfc3339_at(now + elapsed),
            &rfc3339_at(ready_at + elapsed),
        )?
        else {
            return Ok(false);
        };
        self.dependencies
            .store
            .commit_agent_work_repeated_failure(retry, failure.observation.clone())
            .await?;
        tracing::debug!(
            step = agent.work.step.operation_name(),
            attempts = agent.work.attempts.saturating_add(1),
            "Repeated Agent failure updated retry metadata"
        );
        Ok(true)
    }
}
