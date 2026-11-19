//! Bounded transaction retries release every snapshot before backing off.
use std::time::Duration;
use tokio::time::Instant;

#[cfg(test)]
tokio::task_local! {
    pub(super) static RETRY_DEADLINE_ANCHOR:
        Option<std::sync::Arc<std::sync::Mutex<Option<Instant>>>>;
}

pub(super) struct Retries {
    deadline: Instant,
    attempt: u32,
    jitter: u64,
    #[cfg(test)]
    test_deadline_anchor: Option<std::sync::Arc<std::sync::Mutex<Option<Instant>>>>,
    #[cfg(test)]
    test_holder_attempts: u32,
    #[cfg(test)]
    test_stop_reason: &'static str,
}
impl Retries {
    pub(super) fn new() -> Self {
        Self {
            deadline: Instant::now() + Duration::from_secs(5),
            attempt: 0,
            jitter: uuid::Uuid::new_v4().as_u128() as u64,
            #[cfg(test)]
            test_deadline_anchor: RETRY_DEADLINE_ANCHOR.try_with(Clone::clone).ok().flatten(),
            #[cfg(test)]
            test_holder_attempts: 0,
            #[cfg(test)]
            test_stop_reason: "returned",
        }
    }

    fn deadline(&self) -> Option<Instant> {
        #[cfg(test)]
        if let Some(anchor) = &self.test_deadline_anchor {
            // A configured but unset anchor has no wall deadline yet. Attempts
            // and backoff are still spent while the artificial holder is active.
            return anchor
                .lock()
                .unwrap()
                .map(|start| start + Duration::from_secs(5));
        }
        Some(self.deadline)
    }

    pub(super) async fn next(&mut self) -> bool {
        let now = Instant::now();
        let deadline = self.deadline();
        if self.attempt >= 64 || deadline.is_some_and(|deadline| now >= deadline) {
            #[cfg(test)]
            {
                self.test_stop_reason = if self.attempt >= 64 {
                    "attempt_limit"
                } else {
                    "deadline_before_attempt"
                };
            }
            return false;
        }
        if self.attempt > 0 {
            let base = 5_u64 << (self.attempt - 1).min(4);
            let jitter = self.jitter.rotate_left(self.attempt) & 63;
            let delay = Duration::from_millis(base + jitter);
            tokio::time::sleep(deadline.map_or(delay, |deadline| delay.min(deadline - now))).await;
        }
        self.attempt += 1;
        // Re-read after sleeping: the holder may have been released during backoff.
        let deadline = self.deadline();
        #[cfg(test)]
        if deadline.is_none() {
            self.test_holder_attempts = self.attempt;
        }
        #[cfg(test)]
        {
            let next = deadline.is_none_or(|deadline| Instant::now() < deadline);
            if !next {
                self.test_stop_reason = "deadline_after_backoff";
            }
            next
        }
        #[cfg(not(test))]
        deadline.is_none_or(|deadline| Instant::now() < deadline)
    }
}

#[cfg(test)]
impl Drop for Retries {
    fn drop(&mut self) {
        if self.test_deadline_anchor.is_some() {
            let post_release_elapsed_ms = self
                .test_deadline_anchor
                .as_ref()
                .and_then(|anchor| *anchor.lock().unwrap())
                .map(|released| released.elapsed().as_millis());
            eprintln!(
                "maintenance_retry attempts={} holder_attempts={} stop_reason={} post_release_elapsed_ms={post_release_elapsed_ms:?}",
                self.attempt, self.test_holder_attempts, self.test_stop_reason
            );
        }
    }
}
