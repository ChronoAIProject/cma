//! Supervise local scheduling, never infer remote cancellation from local failure.
//!
//! The scheduler's owned state lives outside the unwind boundary. In-flight
//! children, discovery cursors and local deferrals survive a loop restart.
use std::{
    collections::VecDeque,
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex},
    time::Duration,
};

use futures_util::{future::BoxFuture, FutureExt};
use serde::Serialize;
use tokio::{sync::watch, task::JoinHandle, time::Instant};

const WATCH_INTERVAL: Duration = Duration::from_secs(1);
const LOOP_GRACE: Duration = Duration::from_secs(5);
const RESTART_WINDOW: Duration = Duration::from_secs(60);
const MAX_RESTARTS: usize = 5;
pub(crate) const DISCOVERY_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub(crate) struct Workers(Vec<Activity>);
impl Workers {
    pub(crate) fn new() -> Self {
        Self(
            [
                ("launches", true),
                ("agent_commands", true),
                ("sandbox_commands", true),
                ("agents", true),
                ("sandboxes", true),
                ("workspaces", true),
                ("sandbox_reconciliation", true),
                ("agent_capacity_recovery", true),
                ("agent_stale_turn_reconciliation", false),
                ("agent_turn_timeout", true),
                ("native_warm_pools", false),
                #[cfg(test)]
                ("home_backup_expiry", false),
            ]
            .into_iter()
            .map(|(name, required)| Activity::new(name, required))
            .collect(),
        )
    }
    pub(crate) fn get(&self, name: &str) -> Activity {
        self.0
            .iter()
            .find(|a| a.0.lock().unwrap().name == name)
            .expect("known worker")
            .clone()
    }
    pub(crate) fn snapshot(&self) -> Vec<Snapshot> {
        self.0.iter().map(Activity::snapshot).collect()
    }
}

#[derive(Clone)]
pub(crate) struct Activity(Arc<Mutex<State>>);
struct State {
    name: &'static str,
    required: bool,
    status: &'static str,
    phase: &'static str,
    phase_started: Instant,
    last_activity: Option<Instant>,
    expected_wake: Option<Instant>,
    waiting: &'static str,
    discovery_started: Option<Instant>,
    reconciliation_started: Option<Instant>,
    observed_turn: bool,
    #[cfg(test)]
    fault: Option<service_fault_tests::Fault>,
}

impl State {
    fn update(
        &mut self,
        status: &'static str,
        phase: &'static str,
        waiting: &'static str,
        wake: Option<Instant>,
    ) {
        if self.status != status {
            tracing::info!(
                worker = self.name,
                status,
                "worker supervision state changed"
            );
        }
        self.status = status;
        metrics::gauge!("cma_worker_execution_available", "worker" => self.name)
            .set(f64::from(status == "running"));
        if self.phase != phase || self.waiting != waiting {
            self.phase = phase;
            self.phase_started = Instant::now();
        }
        self.waiting = waiting;
        self.expected_wake = wake;
    }
}

#[derive(Serialize)]
pub(crate) struct Snapshot {
    pub(crate) worker: &'static str,
    pub(crate) required: bool,
    pub(crate) state: &'static str,
    phase: &'static str,
    phase_age_ms: u128,
    activity_age_ms: Option<u128>,
    expected_wake_in_ms: Option<u128>,
    waiting: &'static str,
    discovery_age_ms: Option<u128>,
    reconciliation_age_ms: Option<u128>,
}
impl Snapshot {
    pub(crate) fn available(&self) -> bool {
        !self.required || self.state == "running"
    }
}
impl Activity {
    fn new(name: &'static str, required: bool) -> Self {
        Self(Arc::new(Mutex::new(State {
            name,
            required,
            status: "starting",
            phase: "starting",
            phase_started: Instant::now(),
            last_activity: None,
            expected_wake: None,
            waiting: "startup",
            discovery_started: None,
            reconciliation_started: None,
            observed_turn: false,
            #[cfg(test)]
            fault: None,
        })))
    }
    fn update(
        &self,
        status: &'static str,
        phase: &'static str,
        waiting: &'static str,
        wake: Option<Instant>,
    ) {
        self.0.lock().unwrap().update(status, phase, waiting, wake);
    }
    fn begin_attempt(&self) {
        let mut state = self.0.lock().unwrap();
        let now = Instant::now();
        state.observed_turn = false;
        state.last_activity = None;
        // A scheduler can suspend before registering its first bounded wait.
        state.update("starting", "starting", "startup", Some(now));
        state.phase_started = now;
    }
    /// Called only by the actual scheduler, before its next bounded select.
    pub(crate) fn waiting(&self, interval: Duration, reason: &'static str) {
        let mut state = self.0.lock().unwrap();
        let now = Instant::now();
        let status = if state.observed_turn {
            "running"
        } else {
            "starting"
        };
        state.update(status, "scheduling", reason, Some(now + interval));
        state.last_activity = Some(now);
    }
    /// Evidence that the recovered loop has actually driven its select/work
    /// path once. Merely constructing a replacement future is not recovery.
    pub(crate) fn scheduled(&self) {
        self.0.lock().unwrap().observed_turn = true;
    }
    pub(crate) fn reconciliation(&self) -> Reconciliation {
        self.0.lock().unwrap().reconciliation_started = Some(Instant::now());
        Reconciliation(self.clone())
    }
    fn check(&self) -> bool {
        let mut state = self.0.lock().unwrap();
        let now = Instant::now();
        let stalled = state
            .expected_wake
            .is_some_and(|wake| now >= wake + LOOP_GRACE);
        let discovery = state
            .discovery_started
            .is_some_and(|start| now >= start + DISCOVERY_DEADLINE + WATCH_INTERVAL);
        if matches!(state.status, "running" | "starting") && (stalled || discovery) {
            state.status = "suspected_stall";
            tracing::warn!(
                worker = state.name,
                "worker suspected stalled; execution children retain ownership"
            );
        }
        metrics::gauge!("cma_worker_execution_available", "worker" => state.name)
            .set(f64::from(state.status == "running"));
        stalled && matches!(state.status, "suspected_stall")
    }
    pub(crate) fn snapshot(&self) -> Snapshot {
        // Also evaluate freshness when reading so a stopped watchdog cannot
        // leave a formerly healthy worker ready indefinitely.
        self.check();
        let state = self.0.lock().unwrap();
        Snapshot {
            worker: state.name,
            required: state.required,
            state: state.status,
            phase: state.phase,
            phase_age_ms: state.phase_started.elapsed().as_millis(),
            activity_age_ms: state.last_activity.map(|at| at.elapsed().as_millis()),
            expected_wake_in_ms: state
                .expected_wake
                .map(|at| at.saturating_duration_since(Instant::now()).as_millis()),
            waiting: state.waiting,
            discovery_age_ms: state.discovery_started.map(|at| at.elapsed().as_millis()),
            reconciliation_age_ms: state
                .reconciliation_started
                .map(|at| at.elapsed().as_millis()),
        }
    }
}

pub(crate) struct Reconciliation(Activity);
impl Drop for Reconciliation {
    fn drop(&mut self) {
        self.0 .0.lock().unwrap().reconciliation_started = None;
    }
}

/// A separate deadline is permitted only for read-only discovery. Expiration
/// drops the query, releases its slot through the normal child completion path,
/// and never cancels an execution task or resets durable ownership.
pub(crate) async fn discover<T, E>(
    activity: Option<Activity>,
    read: impl std::future::Future<Output = Result<T, E>>,
    timeout_error: E,
) -> Result<T, E> {
    struct Discovery(Option<Activity>);
    impl Drop for Discovery {
        fn drop(&mut self) {
            if let Some(activity) = &self.0 {
                activity.0.lock().unwrap().discovery_started = None;
            }
        }
    }
    if let Some(activity) = &activity {
        activity.0.lock().unwrap().discovery_started = Some(Instant::now());
    }
    let _guard = Discovery(activity);
    tokio::time::timeout(DISCOVERY_DEADLINE, read)
        .await
        .unwrap_or(Err(timeout_error))
}

/// `run` must contain only cancellation-safe scheduling awaits. All effectful
/// operations must be owned children in `owned`; they are never dropped on a
/// loop restart. Current callers select on timers, notifications and JoinSets.
/// `drain` runs once on shutdown, including shutdown during failure backoff,
/// retaining the scheduler's existing child-completion and drain deadlines.
pub(crate) fn spawn<S, F, D>(
    mut owned: S,
    activity: Activity,
    shutdown: watch::Receiver<bool>,
    mut run: F,
    drain: D,
) -> JoinHandle<()>
where
    S: Send + 'static,
    F: for<'a> FnMut(&'a mut S, Activity, watch::Receiver<bool>) -> BoxFuture<'a, ()>
        + Send
        + 'static,
    D: for<'a> FnOnce(&'a mut S) -> BoxFuture<'a, ()> + Send + 'static,
{
    tokio::spawn(async move {
        let mut failures = VecDeque::new();
        let mut backoff = Duration::from_secs(1);
        loop {
            if *shutdown.borrow() || shutdown.has_changed().is_err() {
                break;
            }
            activity.begin_attempt();
            let mut watch = tokio::time::interval(WATCH_INTERVAL);
            watch.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // The unwind/cancellation boundary excludes owned state. Dropping
            // a suspended scheduling future cannot cancel its execution children.
            let reason = {
                let attempt = AssertUnwindSafe(async {
                    let future = run(&mut owned, activity.clone(), shutdown.clone());
                    #[cfg(test)]
                    let future = service_fault_tests::inject(activity.clone(), future);
                    future.await
                })
                .catch_unwind();
                tokio::pin!(attempt);
                loop {
                    tokio::select! {
                        biased;
                        result = &mut attempt => break if result.is_err() { "panic" } else { "exit" },
                        _ = crate::shutdown::signalled(shutdown.clone()) => break "shutdown",
                        _ = watch.tick() => {
                            if *shutdown.borrow() || shutdown.has_changed().is_err() {
                                activity.update("draining", "shutdown", "shutdown", None);
                            } else if activity.check() {
                                break "stall";
                            }
                        },
                    }
                }
            };
            if *shutdown.borrow() || shutdown.has_changed().is_err() {
                break;
            }
            let name = activity.snapshot().worker;
            metrics::counter!("cma_worker_supervision_exits_total", "worker" => name, "reason" => reason).increment(1);
            let now = Instant::now();
            failures.retain(|at| now.duration_since(*at) < RESTART_WINDOW);
            if failures.is_empty() {
                backoff = Duration::from_secs(1);
            }
            failures.push_back(now);
            let (status, delay) = if failures.len() >= MAX_RESTARTS {
                ("restart_limited", RESTART_WINDOW)
            } else {
                ("backoff", backoff)
            };
            activity.update(status, "recovery", status, Some(now + delay));
            tokio::select! {
                biased;
                _ = crate::shutdown::signalled(shutdown.clone()) => break,
                _ = tokio::time::sleep(delay) => {},
            }
            metrics::counter!("cma_worker_restarts_total", "worker" => name).increment(1);
            backoff = (backoff * 2).min(Duration::from_secs(16));
        }
        activity.update("draining", "shutdown", "shutdown", None);
        drain(&mut owned).await;
        activity.update("stopped", "shutdown", "shutdown", None);
        // owned is dropped here. Any JoinSet remains structurally owned even
        // if the outer shutdown coordinator aborts this supervisor.
    })
}

#[cfg(test)]
pub(crate) fn panic_next_poll(activity: &Activity) {
    activity.0.lock().unwrap().fault = Some(service_fault_tests::Fault::Panic);
}

#[cfg(test)]
pub(crate) fn stall_next_poll(activity: &Activity) {
    activity.0.lock().unwrap().fault = Some(service_fault_tests::Fault::Stall);
}

#[cfg(test)]
mod service_fault_tests;
#[cfg(test)]
pub(crate) use service_fault_tests::startup_recovery_with_owned_completion;
#[cfg(test)]
mod tests;
