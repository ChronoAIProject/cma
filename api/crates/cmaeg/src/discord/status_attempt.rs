//! Each handle is fenced to one process-local source revision and attempt.
use super::{
    registry::{timestamp, timestamp_after, Entry},
    FailureCode, Outcome, Stage, StatusRegistry,
};
use std::time::Duration;
use tokio::time::Instant;

#[derive(Clone, Default)]
pub(crate) struct StatusAttempt {
    pub(super) registry: StatusRegistry,
    pub(super) record: String,
    pub(super) generation: u64,
}
impl StatusAttempt {
    /// Only this attempt's failure; retained history cannot select a retry policy.
    pub(crate) fn failure_code(&self) -> Option<FailureCode> {
        self.registry.failure_code(&self.record, self.generation)
    }
    pub(crate) fn allow_diagnostic(&self) -> bool {
        self.registry.allow_diagnostic()
    }
    fn update(&self, update: impl FnOnce(&mut Entry)) {
        self.registry.update(&self.record, self.generation, update);
    }
    pub(crate) fn poll(&self) {
        self.update(|e| e.touch());
    }
    pub(crate) fn step(&self, stage: Stage) {
        self.update(|e| {
            if e.finished || e.failed {
                return;
            }
            e.value.stage = stage;
            e.value.outcome = Outcome::Pending;
            e.touch();
        });
    }
    /// Capture the first specific failure; cleanup cannot replace its cause.
    pub(crate) fn failure(&self, code: FailureCode) {
        self.update(|e| {
            if e.failed || e.finished {
                return;
            }
            e.failed = true;
            // A failed attempt no longer serves, whatever it reached earlier.
            if e.value.outcome == Outcome::Ok {
                e.value.outcome = Outcome::Pending;
            }
            e.value.last_failure_stage = Some(e.value.stage);
            e.value.last_failure_code = Some(code);
            e.value.last_failure_at = timestamp();
            e.disconnect();
            e.touch();
        });
    }
    pub(crate) fn result<T>(
        &self,
        result: Result<T, super::super::GatewayError>,
    ) -> Result<T, super::super::GatewayError> {
        if let Err(error) = &result {
            self.failure((*error).into());
        }
        result
    }
    pub(crate) fn finish(&self, code: FailureCode, terminal: bool, delay: Duration) {
        self.failure(code);
        self.update(|e| {
            e.finished = true;
            e.value.outcome = if terminal {
                Outcome::Blocked
            } else {
                Outcome::Retrying
            };
            e.value.stage = e.value.last_failure_stage.unwrap_or(e.value.stage);
            e.value.retry_at = (!terminal).then(|| timestamp_after(delay)).flatten();
            e.disconnect();
            e.touch();
        });
    }
    /// Re-observing a terminal revision is not a new connection failure.
    pub(crate) fn restore_terminal(&self, snapshot: &super::NativeDiscordConnectionStatus) {
        self.update(|e| {
            e.value = snapshot.clone();
            e.failed = true;
            e.finished = true;
            e.disconnect();
            e.touch();
        });
    }
    pub(crate) fn stopped(&self) {
        self.update(|e| {
            e.finished = true;
            e.value.stage = Stage::Stopped;
            e.value.outcome = Outcome::Stopped;
            e.value.retry_at = None;
            e.disconnect();
            e.touch();
        });
    }
    pub(crate) fn lease_acquired(&self, remaining: Duration) {
        self.update(|e| {
            if e.finished || e.failed {
                return;
            }
            e.value.stage = Stage::Lease;
            e.value.outcome = Outcome::Ok;
            e.value.lease = "acquired".into();
            e.lease_until = Some(Instant::now() + remaining);
            e.touch();
        });
    }
    pub(crate) fn lease_renewed(&self, remaining: Duration) {
        self.update(|e| {
            if e.finished || e.failed {
                return;
            }
            e.lease_until = Some(Instant::now() + remaining);
            e.touch();
        });
    }
    pub(crate) fn lease_released(&self, success: bool) {
        self.update(|e| {
            e.value.lease = if success { "released" } else { "lost" }.into();
            e.lease_until = None;
            e.disconnect();
            e.touch();
        });
    }
    pub(crate) fn lease_refused(&self, code: FailureCode) {
        self.update(|e| {
            e.value.lease = if code == FailureCode::LeaseContended {
                "contended"
            } else {
                "lost"
            }
            .into();
        });
        self.failure(code);
    }
    pub(crate) fn identify_reserved(&self, resume: bool) {
        self.step(if resume {
            Stage::Resume
        } else {
            Stage::Identify
        });
        self.update(|e| {
            if e.finished || e.failed {
                return;
            }
            e.value.identify = if resume { "resuming" } else { "reserved" }.into();
        });
    }
    pub(crate) fn identify_sent(&self) {
        self.update(|e| {
            if e.finished || e.failed {
                return;
            }
            e.value.identify = if e.value.identify == "resuming" {
                "resume_sent"
            } else {
                "sent"
            }
            .into();
            e.touch();
        });
    }
    pub(crate) fn heartbeat_ack(&self, remaining: Duration) {
        self.update(|e| {
            if e.finished || e.failed {
                return;
            }
            // An ACK alone never authenticates a source or refreshes its lease.
            e.heartbeat_until = Some(Instant::now() + remaining);
            e.touch();
        });
    }
    pub(crate) fn ready(&self) {
        self.update(|e| {
            if e.finished || e.failed {
                return;
            }
            e.value.stage = Stage::Ready;
            e.value.outcome = Outcome::Ok;
            e.value.connected = Some(true);
            e.value.authenticated = Some(true);
            e.value.ready = Some(true);
            e.value.last_ready_at = timestamp();
            e.touch();
        });
    }
    pub(crate) fn event_received(&self) {
        self.update(|e| {
            if e.finished || e.value.authenticated != Some(true) {
                return;
            }
            e.value.last_event_at = timestamp();
            e.touch();
        });
    }
    pub(crate) fn event_committed(&self, message: bool) {
        self.update(|e| {
            if e.finished || e.value.authenticated != Some(true) {
                return;
            }
            e.value.stage = Stage::Ready;
            e.value.outcome = Outcome::Ok;
            if message {
                e.value.last_event_committed_at = timestamp();
            }
            e.touch();
        });
    }
    pub(crate) fn cleanup_failed(&self) {
        // Route through the shared process log limit. Never emit errors.
        self.registry.diagnostic(Some(&self.record));
        self.update(|e| {
            e.value.lease = "lost".into();
            if !e.failed {
                e.value.outcome = Outcome::Retrying;
            }
        });
    }
}
