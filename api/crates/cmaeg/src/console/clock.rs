//! The console's notion of now. Session lifetimes, token expiry and the
//! switcher's refetch all read it, so tests can move time without waiting.
//! Owner authority has no clock of its own: it rides REST's creator cache.
use std::time::Instant;

pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

/// The process clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}
