//! Allow contending exact admissions time to progress without changing any
//! transaction, receipt or ambiguous-commit reconciliation boundary.
use std::time::Duration;
use tokio::time::Instant;

pub(super) struct AdmissionRetries {
    deadline: Instant,
    attempt: u32,
    jitter: u64,
}
impl AdmissionRetries {
    pub(super) fn new() -> Self {
        Self {
            deadline: Instant::now() + Duration::from_secs(5),
            attempt: 0,
            jitter: uuid::Uuid::new_v4().as_u128() as u64,
        }
    }
    /// The elapsed budget limits retries between complete attempts. It never
    /// cancels a transaction or abandons an in-flight commit/reconciliation.
    pub(super) async fn next(&mut self) -> bool {
        if self.attempt == 0 {
            self.attempt = 1;
            return true;
        }
        let now = Instant::now();
        if now >= self.deadline {
            return false;
        }
        let backoff = 5_u64 << (self.attempt - 1).min(4);
        let jitter = self.jitter.rotate_left(self.attempt) & 31;
        let delay = Duration::from_millis(backoff + jitter).min(self.deadline - now);
        tokio::time::sleep(delay).await;
        self.attempt = self.attempt.saturating_add(1);
        Instant::now() < self.deadline
    }
}
