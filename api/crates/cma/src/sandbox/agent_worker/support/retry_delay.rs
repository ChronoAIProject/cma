//! Pace failed final filesystem cleanup without delaying live recovery or stop.
use std::time::Duration;

use super::{Agent, AgentWorkStep, AgentWorker};

impl AgentWorker {
    pub(in crate::sandbox::agent_worker) fn failure_retry_delay(
        &self,
        agent: &Agent,
        ledger_hold: bool,
    ) -> Duration {
        if ledger_hold {
            return removal_delay(
                self.config.retry_delay.max(Duration::from_secs(30)),
                agent.work.attempts,
            );
        }
        if agent.work.step != AgentWorkStep::RemoveWorktree {
            return self.config.retry_delay;
        }
        removal_delay(self.config.retry_delay, agent.work.attempts)
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
