//! Bounded transaction retries release every snapshot before backing off.
use std::time::Duration;
use tokio::time::Instant;

pub(super) struct Retries {
    deadline: Instant,
    attempt: u32,
    jitter: u64,
}
impl Retries {
    pub(super) fn new() -> Self {
        Self {
            deadline: Instant::now() + Duration::from_secs(5),
            attempt: 0,
            jitter: uuid::Uuid::new_v4().as_u128() as u64,
        }
    }
    pub(super) async fn next(&mut self) -> bool {
        let now = Instant::now();
        if self.attempt >= 64 || now >= self.deadline {
            return false;
        }
        if self.attempt > 0 {
            let base = 5_u64 << (self.attempt - 1).min(4);
            let jitter = self.jitter.rotate_left(self.attempt) & 63;
            tokio::time::sleep(Duration::from_millis(base + jitter).min(self.deadline - now)).await;
        }
        self.attempt += 1;
        Instant::now() < self.deadline
    }
}
