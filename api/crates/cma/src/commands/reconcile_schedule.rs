//! Bounded process-local pacing, never durable authority or completion evidence.
use super::scheduler::MAX_DEFERRED;
use layer2_core::ids::AgentCommandId;
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::Instant;

pub(super) const RECONCILE_AUDIT_INTERVAL: Duration = Duration::from_secs(300);

pub(super) struct ReconcileSchedule {
    enabled: bool,
    last_fold: HashMap<AgentCommandId, Instant>,
    failures: HashMap<AgentCommandId, (u32, Instant)>,
}

impl ReconcileSchedule {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            last_fold: HashMap::new(),
            failures: HashMap::new(),
        }
    }

    pub(super) fn prune(&mut self, now: Instant) {
        self.last_fold
            .retain(|_, folded| now.duration_since(*folded) < RECONCILE_AUDIT_INTERVAL);
        self.failures
            .retain(|_, (_, failed)| now.duration_since(*failed) < RECONCILE_AUDIT_INTERVAL);
    }

    pub(super) fn current(&self, id: &AgentCommandId, now: Instant) -> bool {
        self.enabled
            && self
                .last_fold
                .get(id)
                .is_some_and(|folded| now.duration_since(*folded) < RECONCILE_AUDIT_INTERVAL)
    }

    pub(super) fn succeeded(&mut self, id: &AgentCommandId, now: Instant) {
        self.failures.remove(id);
        if self.enabled && (self.last_fold.len() < MAX_DEFERRED || self.last_fold.contains_key(id))
        {
            self.last_fold.insert(id.clone(), now);
        }
    }

    pub(super) fn failed(&mut self, id: &AgentCommandId, now: Instant) -> Duration {
        // A failed audit must be retried even if a previous fold is recent.
        self.last_fold.remove(id);
        if !self.enabled {
            return Duration::ZERO;
        }
        let failures = self
            .failures
            .get(id)
            .map_or(1, |(count, _)| count.saturating_add(1).min(6));
        if self.failures.len() < MAX_DEFERRED || self.failures.contains_key(id) {
            self.failures.insert(id.clone(), (failures, now));
        }
        Duration::from_millis((250_u64 << (failures - 1)).min(5_000))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn full_caches_require_folds_and_expire_without_growing() {
        let mut schedule = ReconcileSchedule::new(true);
        let now = Instant::now();
        for index in 0..=MAX_DEFERRED {
            schedule.succeeded(&AgentCommandId(format!("cmd_{index}")), now);
        }
        assert_eq!(schedule.last_fold.len(), MAX_DEFERRED);
        assert!(!schedule.current(&AgentCommandId(format!("cmd_{MAX_DEFERRED}")), now));
        for index in 0..=MAX_DEFERRED {
            assert_eq!(
                schedule.failed(&AgentCommandId(format!("cmd_{index}")), now),
                Duration::from_millis(250)
            );
        }
        assert!(schedule.last_fold.is_empty());
        assert_eq!(schedule.failures.len(), MAX_DEFERRED);
        tokio::time::advance(RECONCILE_AUDIT_INTERVAL).await;
        schedule.prune(Instant::now());
        assert!(schedule.failures.is_empty());
        schedule.succeeded(&AgentCommandId("new".into()), Instant::now());
        assert!(schedule.current(&AgentCommandId("new".into()), Instant::now()));
    }
}
