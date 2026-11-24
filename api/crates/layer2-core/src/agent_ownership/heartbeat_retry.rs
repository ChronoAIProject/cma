use super::{OwnershipError, HEARTBEAT_INTERVAL_MS, STORE_DEADLINE_MS};
use std::time::Duration;
use tokio::time::Instant;

pub(super) struct RetryState {
    generation: u64,
    pub episode: Option<Episode>,
    pub next_at: Instant,
}

pub(super) struct Episode {
    generation: u64,
    pub started: Instant,
    pub confirmed_until: Instant,
    pub failures: u32,
    final_attempted: bool,
}

/// Only a coordinator episode that observed a typed transient failure mints this.
#[derive(Clone, Copy)]
pub(super) struct TransientEpisode {
    generation: u64,
}

impl TransientEpisode {
    pub(super) fn matches(self, generation: u64) -> bool {
        self.generation == generation
    }
}

impl Episode {
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    pub(super) fn proof(&self) -> Option<TransientEpisode> {
        (self.failures > 0).then_some(TransientEpisode {
            generation: self.generation,
        })
    }

    pub(super) fn attempt_until(&mut self, now: Instant) -> Option<Instant> {
        if self.final_attempted || now >= self.confirmed_until {
            return None;
        }
        let full = now + Duration::from_millis(STORE_DEADLINE_MS);
        if full > self.confirmed_until {
            self.final_attempted = true;
        }
        Some(full.min(self.confirmed_until))
    }

    pub(super) fn unavailable(&mut self, now: Instant) -> Instant {
        self.failures = self.failures.saturating_add(1);
        if self.final_attempted {
            return self.confirmed_until;
        }
        let spacing = Duration::from_millis(match self.failures {
            1 => 250,
            2 => 500,
            _ => 1_000,
        });
        let final_window = self.confirmed_until - Duration::from_millis(250);
        (now + spacing).min(final_window).max(now)
    }
}

impl RetryState {
    pub(super) fn new() -> Self {
        Self {
            generation: 0,
            episode: None,
            next_at: Instant::now(),
        }
    }

    pub(super) fn begin(
        &mut self,
        confirmed_until: Instant,
    ) -> Result<&mut Episode, OwnershipError> {
        if self.episode.is_none() {
            self.generation = self
                .generation
                .checked_add(1)
                .ok_or_else(|| OwnershipError::Invalid("heartbeat episode exhausted".into()))?;
            self.episode = Some(Episode {
                generation: self.generation,
                started: Instant::now(),
                confirmed_until,
                failures: 0,
                final_attempted: false,
            });
        }
        Ok(self.episode.as_mut().unwrap())
    }

    pub(super) fn confirmed(&mut self, now: Instant) {
        self.episode = None;
        self.next_at = now + Duration::from_millis(HEARTBEAT_INTERVAL_MS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn instantaneous_failures_have_spacing_and_one_persisted_final_attempt() {
        let started = Instant::now();
        let until = started + Duration::from_secs(15);
        let mut retry = RetryState::new();
        let mut starts = Vec::new();
        let mut shortened = 0;
        loop {
            tokio::time::sleep_until(retry.next_at).await;
            let now = Instant::now();
            if now >= until {
                break;
            }
            let episode = retry.begin(until).unwrap();
            let call_until = episode
                .attempt_until(now)
                .expect("only the old deadline may end retries");
            starts.push(now);
            shortened += usize::from(call_until - now < Duration::from_millis(STORE_DEADLINE_MS));
            retry.next_at = episode.unavailable(now);
            // The episode survives every returned RetryAt, including the final latch.
            assert_eq!(retry.begin(until).unwrap().generation(), 1);
        }
        assert_eq!(Instant::now(), until);
        assert_eq!(shortened, 1);
        assert!(starts.windows(2).all(|pair| pair[1] > pair[0]));
        assert_eq!(starts[1] - starts[0], Duration::from_millis(250));
        assert_eq!(starts[2] - starts[1], Duration::from_millis(500));
        assert!(starts[2..]
            .windows(2)
            .all(|pair| pair[1] - pair[0] <= Duration::from_secs(1)));
        let maximum = 3 + ((until - started).as_millis() - 750).div_ceil(1_000) as usize;
        assert!(starts.len() <= maximum);
        assert!(retry.begin(until).unwrap().attempt_until(until).is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn final_immediate_failure_waits_for_deadline_instead_of_failing_early() {
        let mut retry = RetryState::new();
        let now = Instant::now();
        let until = now + Duration::from_millis(200);
        let episode = retry.begin(until).unwrap();
        assert!(episode.proof().is_none());
        assert_eq!(episode.attempt_until(now), Some(until));
        assert_eq!(episode.unavailable(now), until);
        assert!(episode.proof().unwrap().matches(1));
        assert_eq!(retry.begin(until).unwrap().attempt_until(now), None);
    }
}
