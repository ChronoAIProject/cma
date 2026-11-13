//! Preserve legacy retry floors while sharing durable failure budgets.
use layer2_core::sandbox::work_retry::{work_retry_policy, WorkRetryDecision, WorkRetryFailure};
use std::time::Duration;

use super::{Agent, AgentWorkStep, AgentWorker, WorkerFailure};

impl AgentWorker {
    pub(in crate::sandbox::agent_worker) fn failure_retry_delay(
        &self,
        agent: &Agent,
        failure: &WorkerFailure,
    ) -> Duration {
        self.retry_decision(agent, failure, self.config.retry_delay)
            .delay
    }

    pub(in crate::sandbox::agent_worker) fn retry_decision(
        &self,
        agent: &Agent,
        failure: &WorkerFailure,
        base: Duration,
    ) -> WorkRetryDecision {
        let legacy = if failure.is_ledger_hold() {
            removal_delay(base.max(Duration::from_secs(30)), agent.work.attempts)
        } else if agent.work.step == AgentWorkStep::RemoveWorktree {
            removal_delay(base, agent.work.attempts)
        } else {
            base
        };
        if !self.config.work_retry_policy_enabled {
            return WorkRetryDecision {
                delay: legacy,
                parked: false,
            };
        }
        let mut decision = work_retry_policy(
            crate::sandbox::work_retry::agent_class(agent.work.step),
            failure.retry_kind,
            agent.work.attempts,
            base,
        );
        // Existing cleanup/ledger protection may be slower than an interactive
        // grace period. Enabling this policy must never accelerate those paths.
        decision.delay = decision.delay.max(legacy);
        decision
    }
}

impl WorkerFailure {
    pub(in crate::sandbox::agent_worker) fn retry_kind(&self) -> WorkRetryFailure {
        self.retry_kind
    }
}

fn removal_delay(base: Duration, attempts: u32) -> Duration {
    // Attempts are durable and reset when the lifecycle advances. A restart or
    // another worker therefore cannot reset the backoff for unchanged work.
    // Keep the first retry unchanged. Never shorten a configured base delay.
    base.saturating_mul(1_u32 << attempts.min(4))
        .min(Duration::from_secs(300).max(base))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removal_backoff_keeps_first_retry_and_has_a_five_minute_ceiling() {
        let base = Duration::from_secs(30);
        let delays: Vec<_> = (0..7)
            .map(|attempt| removal_delay(base, attempt).as_secs())
            .collect();
        assert_eq!(delays, [30, 60, 120, 240, 300, 300, 300]);
        assert_eq!(removal_delay(base, u32::MAX), Duration::from_secs(300));
        assert_eq!(
            removal_delay(Duration::from_secs(600), 5),
            Duration::from_secs(600)
        );
    }
}
