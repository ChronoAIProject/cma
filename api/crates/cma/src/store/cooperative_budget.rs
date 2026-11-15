//! A process-injected time budget for CPU loops, independent of Mongo.
use super::execution_sections::{Clock, ExecutionSections, SectionStats};
use std::{
    future::Future,
    time::{Duration, Instant},
};

const SECTION_BUDGET: Duration = Duration::from_millis(2);
const CLOCK_STRIDE: u8 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Policy {
    pub(super) enabled: bool,
    pub(super) section_budget: Duration,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            enabled: false,
            section_budget: SECTION_BUDGET,
        }
    }
}
impl Policy {
    pub(super) fn enabled(enabled: bool) -> Self {
        Self {
            enabled,
            ..Self::default()
        }
    }
}

pub(super) struct CooperativeBudget<C> {
    execution: ExecutionSections<C>,
    policy: Policy,
    checkpoints: u8,
    yields: u64,
}
impl<C: Clock> CooperativeBudget<C> {
    pub(super) fn new(policy: Policy, clock: C) -> Self {
        Self {
            execution: ExecutionSections::new(clock),
            policy,
            checkpoints: 0,
            yields: 0,
        }
    }
    pub(super) fn should_yield(&self, now: Instant) -> bool {
        self.policy.enabled
            && self.execution.started_at().is_some_and(|started| {
                now.saturating_duration_since(started) >= self.policy.section_budget
            })
    }
    pub(super) async fn observe<F: Future + Send>(
        &mut self,
        future: F,
        record_poll: bool,
    ) -> F::Output {
        self.execution.observe(future, record_poll).await
    }
    pub(super) async fn checkpoint(&mut self) {
        if !self.policy.enabled {
            return;
        }
        self.checkpoints += 1;
        if self.checkpoints < CLOCK_STRIDE {
            return;
        }
        self.checkpoints = 0;
        let now = self.execution.now();
        if self.should_yield(now) {
            self.execution.suspend(now);
            self.yields = self.yields.saturating_add(1);
            tokio::task::yield_now().await;
            self.execution.resume(self.execution.now());
        }
    }
    pub(super) fn finish(&mut self) -> (SectionStats, u64) {
        (self.execution.finish(), self.yields)
    }
}

#[cfg(test)]
#[path = "cooperative_budget_tests.rs"]
mod tests;
