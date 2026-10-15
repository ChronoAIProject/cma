//! Give competing command admissions time to commit without extending authority.
use std::time::Duration;
use tokio::time::Instant;

pub(super) struct CommandAdmissionRetries {
    deadline: Instant,
    attempt: u32,
    jitter: u64,
}

impl CommandAdmissionRetries {
    pub(super) fn new(authority_deadline: Option<Instant>) -> Self {
        Self::with_deadline(authority_deadline, None)
    }

    pub(super) fn with_deadline(
        authority_deadline: Option<Instant>,
        retry_deadline: Option<Instant>,
    ) -> Self {
        let ceiling = Instant::now() + Duration::from_secs(5);
        let budget = retry_deadline.map_or(ceiling, |deadline| deadline.min(ceiling));
        Self {
            deadline: authority_deadline.map_or(budget, |deadline| deadline.min(budget)),
            attempt: 0,
            jitter: uuid::Uuid::new_v4().as_u128() as u64,
        }
    }

    // Wait only between complete, retryable attempts. Never cancel an active
    // transaction or interrupt ambiguous-commit handling to enforce this budget.
    pub(super) async fn next(&mut self) -> bool {
        let now = Instant::now();
        if now >= self.deadline {
            return false;
        }
        if self.attempt == 0 {
            self.attempt = 1;
            return true;
        }
        let backoff = 5_u64 << (self.attempt - 1).min(4);
        let jitter = self.jitter.rotate_left(self.attempt) & 31;
        let delay = Duration::from_millis(backoff + jitter).min(self.deadline - now);
        tokio::time::sleep(delay).await;
        self.attempt = self.attempt.saturating_add(1);
        Instant::now() < self.deadline
    }
}
