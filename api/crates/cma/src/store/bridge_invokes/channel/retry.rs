//! Per-Channel contention budget; the original proof is never refreshed.
use std::time::Duration;
use tokio::time::Instant;

pub(super) struct ChannelAdmissionRetries {
    deadline: Instant,
    attempt: u32,
    jitter: u64,
}

impl ChannelAdmissionRetries {
    pub(super) fn new(authority_deadline: Instant) -> Self {
        Self {
            deadline: authority_deadline.min(Instant::now() + Duration::from_secs(5)),
            attempt: 0,
            jitter: uuid::Uuid::new_v4().as_u128() as u64,
        }
    }

    /// Only waits between complete, proven-retryable attempts. This does not
    /// cancel a transaction or abandon an in-flight commit/reconciliation.
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
