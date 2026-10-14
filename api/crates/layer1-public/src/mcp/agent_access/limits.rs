use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

const WINDOW: Duration = Duration::from_secs(60);
const REQUESTS: u16 = 60;
const IDENTITIES: usize = 1024;

#[derive(Default)]
pub(super) struct RateLimit(Mutex<HashMap<[u8; 32], (Instant, u16)>>);

impl RateLimit {
    pub(super) fn admit(&self, capability: &str) -> bool {
        self.admit_at(capability, Instant::now())
    }

    fn admit_at(&self, capability: &str, now: Instant) -> bool {
        let digest: [u8; 32] = Sha256::digest(capability.as_bytes()).into();
        let Ok(mut windows) = self.0.lock() else {
            return false;
        };
        windows.retain(|_, (started, _)| now.saturating_duration_since(*started) < WINDOW);
        if !windows.contains_key(&digest) && windows.len() >= IDENTITIES {
            return false;
        }
        let (_, used) = windows.entry(digest).or_insert((now, 0));
        if *used >= REQUESTS {
            return false;
        }
        *used += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn limits_are_bounded_per_identity_and_recover_after_window() {
        let limiter = RateLimit::default();
        let now = Instant::now();
        for _ in 0..REQUESTS {
            assert!(limiter.admit_at("identity-a", now));
        }
        assert!(!limiter.admit_at("identity-a", now));
        assert!(limiter.admit_at("identity-b", now));
        assert!(limiter.admit_at("identity-a", now + WINDOW));
        for index in 0..IDENTITIES - 1 {
            assert!(limiter.admit_at(&index.to_string(), now + WINDOW));
        }
        assert!(!limiter.admit_at("overflow", now + WINDOW));
        assert_eq!(limiter.0.lock().unwrap().len(), IDENTITIES);
    }
}
