use std::collections::VecDeque;
use std::time::Duration;

use tokio::time::Instant;

const FAILURE_WINDOW: Duration = Duration::from_secs(600);
const STABLE_PERIOD: Duration = Duration::from_secs(60);
const FAILURE_LIMIT: usize = 5;
const FIRST_DELAY: Duration = Duration::from_secs(1);
const MAX_DELAY: Duration = Duration::from_secs(16);

pub(super) struct RestartBudget {
    recent: VecDeque<Instant>,
    next_delay: Duration,
}

impl RestartBudget {
    pub(super) fn new() -> Self {
        Self {
            recent: VecDeque::new(),
            next_delay: FIRST_DELAY,
        }
    }

    /// A healthy period restores a fast retry while the failure window retains
    /// its history. Once exhausted, this desktop generation sequence ends.
    pub(super) fn failure(
        &mut self,
        now: Instant,
        ready_since: Option<Instant>,
    ) -> Option<Duration> {
        if self.recent.len() == FAILURE_LIMIT {
            return None;
        }
        while self
            .recent
            .front()
            .is_some_and(|failure| now.duration_since(*failure) >= FAILURE_WINDOW)
        {
            self.recent.pop_front();
        }
        self.recent.push_back(now);
        if self.recent.len() == FAILURE_LIMIT {
            return None;
        }
        if ready_since.is_some_and(|since| now.duration_since(since) >= STABLE_PERIOD) {
            self.next_delay = FIRST_DELAY;
        }
        let delay = self.next_delay;
        self.next_delay = (2 * delay).min(MAX_DELAY);
        Some(delay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn five_failures_exhaust_the_window_despite_a_healthy_minute_each_time() {
        let mut budget = RestartBudget::new();
        for attempt in 0..5 {
            let ready = Instant::now();
            tokio::time::advance(Duration::from_secs(61)).await;
            let result = budget.failure(Instant::now(), Some(ready));
            assert_eq!(result, if attempt < 4 { Some(FIRST_DELAY) } else { None });
            tokio::time::advance(FIRST_DELAY).await;
        }
        tokio::time::advance(FAILURE_WINDOW).await;
        assert_eq!(budget.failure(Instant::now(), None), None);
    }

    #[tokio::test(start_paused = true)]
    async fn old_failures_expire_at_the_window_boundary() {
        let mut budget = RestartBudget::new();
        for expected in [1, 2, 4, 8] {
            assert_eq!(
                budget.failure(Instant::now(), None),
                Some(Duration::from_secs(expected))
            );
            tokio::time::advance(FIRST_DELAY).await;
        }
        tokio::time::advance(FAILURE_WINDOW - 4 * FIRST_DELAY).await;
        assert_eq!(budget.failure(Instant::now(), None), Some(MAX_DELAY));
        assert_eq!(budget.failure(Instant::now(), None), None);
    }
}
