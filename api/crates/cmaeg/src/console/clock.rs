//! The console's notion of now. Session lifetimes, token ages and authority
//! verdicts all read it, so tests can move time without waiting.
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
