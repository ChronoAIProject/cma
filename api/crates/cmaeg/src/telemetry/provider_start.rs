//! Opt-in, process-local evidence. Never consulted for admission, retry or recovery.
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

const MAX_SAMPLES: usize = 8192;
const WINDOW: Duration = Duration::from_secs(2 * 60 * 60);

#[derive(Clone, Debug)]
pub struct Measurement(Arc<Inner>);
#[derive(Debug)]
struct Inner {
    origin: Instant,
    process: String,
    capacity: usize,
    state: Mutex<State>,
}
#[derive(Debug, Default)]
struct State {
    rows: BTreeMap<String, Sample>,
    overflow: bool,
    active: u64,
    peak: u64,
    executor_attempts: u64,
    busy_ns: u64,
    last_active_ns: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct Sample {
    pub sample: u64,
    pub offered_ns: Option<u64>,
    pub candidate_admitted_at_seconds: Option<i64>,
    pub commit_lower_ns: Option<u64>,
    pub admission_confirmed_ns: Option<u64>,
    pub admitted: bool,
    pub target_count: usize,
    pub executor_started_ns: Option<u64>,
    pub first_post_ns: Option<u64>,
    pub post_attempts: u64,
    pub accepted_ns: Option<u64>,
    pub accepted_attempt_post_ns: Option<u64>,
    pub completed_ns: Option<u64>,
    pub expired_before_start_ns: Option<u64>,
    pub expired_after_post_ns: Option<u64>,
    pub cancellations: u64,
    pub unknown: Option<&'static str>,
}
#[derive(Serialize)]
pub struct Snapshot {
    pub schema_version: u8,
    pub measurement_enabled: bool,
    pub clock: &'static str,
    pub clock_resolution_ns: u64,
    pub process: String,
    pub window_ns: u64,
    pub capacity: usize,
    pub overflow: bool,
    pub window_exceeded: bool,
    pub active_executors: u64,
    pub peak_executors: u64,
    pub executor_attempts: u64,
    pub executor_busy_ns: u64,
    pub samples: Vec<Sample>,
}
impl Measurement {
    /// No production constructor enables this. A local qualification explicitly
    /// attaches the same bounded observer to its PlanStore before creating services.
    pub fn new(capacity: usize) -> Result<Self, &'static str> {
        if !(1..=MAX_SAMPLES).contains(&capacity) {
            return Err("invalid measurement capacity");
        }
        Ok(Self(Arc::new(Inner {
            origin: Instant::now(),
            process: uuid::Uuid::new_v4().to_string(),
            capacity,
            state: Mutex::new(State::default()),
        })))
    }
    pub(crate) fn now(&self) -> u64 {
        self.elapsed(Instant::now())
    }
    fn elapsed(&self, at: Instant) -> u64 {
        at.saturating_duration_since(self.0.origin)
            .as_nanos()
            .try_into()
            .unwrap_or(u64::MAX)
    }
    fn update(&self, id: &str, f: impl FnOnce(&mut Sample, u64)) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = self.now();
        if now > WINDOW.as_nanos() as u64 {
            return;
        }
        if !state.rows.contains_key(id) {
            if state.rows.len() == self.0.capacity {
                state.overflow = true;
                return;
            }
            let sample = state.rows.len() as u64;
            state.rows.insert(
                id.into(),
                Sample {
                    sample,
                    offered_ns: None,
                    candidate_admitted_at_seconds: None,
                    commit_lower_ns: None,
                    admission_confirmed_ns: None,
                    admitted: false,
                    target_count: 0,
                    executor_started_ns: None,
                    first_post_ns: None,
                    post_attempts: 0,
                    accepted_ns: None,
                    accepted_attempt_post_ns: None,
                    completed_ns: None,
                    expired_before_start_ns: None,
                    expired_after_post_ns: None,
                    cancellations: 0,
                    unknown: None,
                },
            );
        }
        f(state.rows.get_mut(id).unwrap(), now);
    }
    pub(crate) fn offered(&self, id: &str) {
        self.update(id, |row, now| {
            if row.offered_ns.is_none() {
                row.offered_ns = Some(now);
                count("offered");
            }
        });
    }
    pub(crate) fn admitted(
        &self,
        id: &str,
        candidate_seconds: i64,
        targets: usize,
        lower: Option<u64>,
    ) {
        self.update(id, |row, now| {
            if !row.admitted {
                row.admitted = true;
                row.candidate_admitted_at_seconds = Some(candidate_seconds);
                row.target_count = targets;
                row.commit_lower_ns = lower;
                row.admission_confirmed_ns = lower.map(|_| now);
                if lower.is_none() {
                    row.unknown = Some("retained_or_uncertain_admission");
                }
                if targets != 1 {
                    row.unknown = Some("single_target_required");
                }
                count("admitted");
            }
        });
    }
    pub(crate) fn attempt(&self, id: &str) -> Attempt {
        Attempt {
            measurement: self.clone(),
            id: id.into(),
            written: Arc::new(OnceLock::new()),
        }
    }
    pub(crate) fn completed(&self, id: &str) {
        self.update(id, |row, now| {
            if row.completed_ns.is_none() {
                row.completed_ns = Some(now);
                count("completed");
            }
        });
    }
    pub(crate) fn expired(&self, id: &str, never_prepared: bool) {
        self.update(id, |row, now| {
            let (field, outcome) = if row.first_post_ns.is_some() {
                (&mut row.expired_after_post_ns, "expired_after_post")
            } else if never_prepared
                && row.commit_lower_ns.is_some()
                && row.admission_confirmed_ns.is_some()
                && row.unknown.is_none()
            {
                (&mut row.expired_before_start_ns, "expired_before_start")
            } else {
                row.unknown
                    .get_or_insert(if row.admission_confirmed_ns.is_some() {
                        "first_post_observation_missing"
                    } else {
                        "retained_or_uncertain_admission"
                    });
                return;
            };
            if field.is_none() {
                *field = Some(now);
                count(outcome);
            }
        });
    }
    pub(crate) fn unknown(&self, id: &str, reason: &'static str) {
        self.update(id, |row, _| {
            row.unknown.get_or_insert(reason);
        });
    }
    pub(crate) fn executor(&self, id: &str) -> ExecutorGuard {
        self.update(id, |row, now| {
            row.executor_started_ns.get_or_insert(now);
        });
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        Self::account(&mut state, self.now());
        state.active += 1;
        state.peak = state.peak.max(state.active);
        state.executor_attempts += 1;
        metrics::gauge!(super::names::PROVIDER_START_ACTIVE).increment(1.0);
        ExecutorGuard {
            measurement: self.clone(),
            id: id.into(),
            finished: false,
        }
    }
    fn account(state: &mut State, now: u64) {
        state.busy_ns = state.busy_ns.saturating_add(
            now.saturating_sub(state.last_active_ns)
                .saturating_mul(state.active),
        );
        state.last_active_ns = now;
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = self.now();
        Self::account(&mut state, now);
        let mut samples: Vec<_> = state.rows.values().cloned().collect();
        samples.sort_by_key(|row| row.sample);
        Snapshot {
            schema_version: 2,
            measurement_enabled: true,
            clock: "process_monotonic_ns",
            clock_resolution_ns: 1_000_000,
            process: self.0.process.clone(),
            window_ns: now,
            capacity: self.0.capacity,
            overflow: state.overflow,
            window_exceeded: now > WINDOW.as_nanos() as u64,
            active_executors: state.active,
            peak_executors: state.peak,
            executor_attempts: state.executor_attempts,
            executor_busy_ns: state.busy_ns,
            samples,
        }
    }
}
#[derive(Clone, Debug)]
pub(crate) struct Attempt {
    measurement: Measurement,
    id: String,
    written: Arc<OnceLock<u64>>,
}
impl Attempt {
    pub(crate) fn written(&self, at: Instant) {
        let at = self.measurement.elapsed(at);
        if self.written.set(at).is_err() {
            return;
        }
        self.measurement.update(&self.id, |row, _| {
            row.post_attempts += 1;
            if row.first_post_ns.is_none() {
                row.first_post_ns = Some(at);
                count("first_started");
            }
        });
    }
    pub(crate) fn accepted(&self) {
        self.measurement.update(&self.id, |row, now| {
            if row.accepted_ns.is_none() {
                row.accepted_ns = Some(now);
                row.accepted_attempt_post_ns = self.written.get().copied();
                count("accepted");
            }
        });
    }
}
pub(crate) struct ExecutorGuard {
    measurement: Measurement,
    id: String,
    finished: bool,
}
impl ExecutorGuard {
    pub(crate) fn finished(&mut self) {
        self.finished = true;
    }
}
impl Drop for ExecutorGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.measurement.update(&self.id, |row, _| {
                row.cancellations += 1;
            });
        }
        let mut state = self
            .measurement
            .0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        Measurement::account(&mut state, self.measurement.now());
        state.active = state.active.saturating_sub(1);
        metrics::gauge!(super::names::PROVIDER_START_ACTIVE).decrement(1.0);
    }
}
fn count(outcome: &'static str) {
    metrics::counter!(super::names::PROVIDER_START_OBSERVATIONS, super::names::OUTCOME => outcome)
        .increment(1);
}

#[cfg(test)]
mod tests;
