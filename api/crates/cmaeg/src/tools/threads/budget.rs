//! One request deadline fits the retained ten-second runtime proxy.
use cmaeg_core::thread_lifecycle::ThreadOutcome;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::time::Instant;

pub(super) const TOTAL: Duration = Duration::from_secs(8);
pub(super) struct Budget {
    pub end: Instant,
    reserve: Duration,
    dispatched: AtomicBool,
    ledger_started: AtomicBool,
}
impl Budget {
    pub fn new(duration: Duration) -> Self {
        Self {
            end: Instant::now() + duration,
            reserve: duration / 4,
            dispatched: AtomicBool::new(false),
            ledger_started: AtomicBool::new(false),
        }
    }
    pub fn preflight_end(&self) -> Instant {
        self.end - self.reserve
    }
    pub fn write_end(&self) -> Instant {
        self.end - self.reserve / 2
    }
    /// No detached writer: this check and the request future share the outer deadline.
    pub fn dispatch(&self) -> Result<(), ThreadOutcome> {
        if self.end.saturating_duration_since(Instant::now()) < self.reserve {
            return Err(ThreadOutcome::Rejected {
                reason: "thread_deadline_before_dispatch".into(),
            });
        }
        self.dispatched.store(true, Ordering::SeqCst);
        Ok(())
    }
    pub fn start_ledger(&self) {
        self.ledger_started.store(true, Ordering::SeqCst);
    }
    pub fn has_dispatched(&self) -> bool {
        self.dispatched.load(Ordering::SeqCst)
    }
    pub fn expired(&self) -> ThreadOutcome {
        if self.has_dispatched() {
            ThreadOutcome::Unknown {
                reason: "thread_deadline_after_dispatch".into(),
            }
        } else if self.ledger_started.load(Ordering::SeqCst) {
            ThreadOutcome::Unknown {
                reason: "thread_admission_or_cleanup_unconfirmed".into(),
            }
        } else {
            ThreadOutcome::Rejected {
                reason: "thread_deadline_before_dispatch".into(),
            }
        }
    }
}
