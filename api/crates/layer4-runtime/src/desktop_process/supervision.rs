use std::time::Duration;

use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::restart_budget::RestartBudget;
use super::transport::{authenticate, DISPLAY_URL};
use super::{DesktopGeneration, DesktopProcess, DesktopUnavailable, STARTUP_LIMIT};
use crate::config::RuntimeConfig;

enum State {
    Starting,
    Ready(DesktopGeneration),
    Restarting,
    Failed,
}

enum ProcessOwner {
    Starting(Box<DesktopProcess>),
    Running(JoinHandle<()>),
    Stopped,
}

pub(crate) struct Supervision {
    config: RuntimeConfig,
    state: watch::Sender<State>,
    shutdown: watch::Sender<bool>,
    owner: Mutex<ProcessOwner>,
}

impl Supervision {
    pub(super) fn new(config: RuntimeConfig, initial: DesktopProcess) -> Self {
        Self {
            config,
            state: watch::Sender::new(State::Starting),
            shutdown: watch::Sender::new(false),
            owner: Mutex::new(ProcessOwner::Starting(Box::new(initial))),
        }
    }

    pub(super) fn generation(&self) -> Result<DesktopGeneration, DesktopUnavailable> {
        match &*self.state.borrow() {
            State::Ready(generation) if generation.active() => Ok(generation.clone()),
            State::Failed => Err(DesktopUnavailable { retryable: false }),
            _ => Err(DesktopUnavailable { retryable: true }),
        }
    }

    pub(super) async fn ready(&self) {
        let mut owner = self.owner.lock().await;
        if !matches!(*owner, ProcessOwner::Starting(_)) {
            return;
        }
        let ProcessOwner::Starting(initial) = std::mem::replace(&mut *owner, ProcessOwner::Stopped)
        else {
            unreachable!("desktop startup ownership checked")
        };
        let config = self.config.clone();
        let mut driver = Recovery {
            state: self.state.clone(),
            shutdown: self.shutdown.subscribe(),
            budget: RestartBudget::new(),
        };
        *owner = ProcessOwner::Running(tokio::spawn(async move {
            driver
                .run(*initial, || DesktopProcess::start(&config), DISPLAY_URL)
                .await;
        }));
        let mut state = self.state.subscribe();
        let _ = state
            .wait_for(|state| !matches!(state, State::Starting))
            .await;
    }

    pub(super) async fn shutdown(&self) {
        self.shutdown.send_replace(true);
        let mut owner = self.owner.lock().await;
        match std::mem::replace(&mut *owner, ProcessOwner::Stopped) {
            ProcessOwner::Running(task) => {
                if let Err(error) = task.await {
                    tracing::error!(%error, "desktop supervisor task failed");
                }
            }
            ProcessOwner::Starting(mut child) => {
                if let Err(error) = child.finish(false).await {
                    tracing::error!(%error, "initial desktop cleanup failed");
                    child.fence();
                }
            }
            ProcessOwner::Stopped => {}
        }
    }
}

impl Drop for Supervision {
    fn drop(&mut self) {
        self.shutdown.send_replace(true);
    }
}

struct Recovery {
    state: watch::Sender<State>,
    shutdown: watch::Receiver<bool>,
    budget: RestartBudget,
}

enum End {
    Exit,
    StartupTimeout,
    Shutdown,
    Uncertain(String),
}

impl Recovery {
    async fn run(
        &mut self,
        initial: DesktopProcess,
        mut launch: impl FnMut() -> crate::failure::Result<DesktopProcess>,
        display: &str,
    ) {
        let mut next = Ok(initial);
        let mut attempt = 0_u64;
        loop {
            let retry = match next {
                Ok(mut child) => {
                    let (end, ready_since) = self.drive(&mut child, attempt, display).await;
                    let failed_at = Instant::now();
                    let delay = match &end {
                        End::Exit | End::StartupTimeout => {
                            self.record_failure(failed_at, ready_since)
                        }
                        _ => None,
                    };
                    if delay.is_some() || matches!(end, End::Shutdown) {
                        self.state.send_replace(State::Restarting);
                    }
                    let result = match &end {
                        End::Uncertain(error) => Err(error.clone()),
                        _ => child.finish(matches!(end, End::Exit)).await,
                    };
                    if let Err(error) = result {
                        self.state.send_replace(State::Failed);
                        tracing::error!(%error, attempt, "desktop cleanup could not be proved");
                        child.fence();
                        // Retain the unreaped leader in this failed state.
                        stopped(self.shutdown.clone()).await;
                        return;
                    }
                    match end {
                        End::Shutdown => return,
                        End::Exit => tracing::warn!(
                            attempt,
                            elapsed_ms = ready_since
                                .map(|start| failed_at.duration_since(start).as_millis()),
                            "desktop generation exited"
                        ),
                        End::StartupTimeout => {
                            tracing::warn!(attempt, "replacement desktop failed readiness")
                        }
                        End::Uncertain(_) => unreachable!("uncertain cleanup already fenced"),
                    }
                    delay
                }
                Err(error) => {
                    tracing::warn!(%error, attempt, "replacement desktop did not start");
                    self.record_failure(Instant::now(), None)
                }
            };

            let Some(delay) = retry else { return };
            if !wait_to_retry(self.shutdown.clone(), delay).await {
                return;
            }
            // Holding the watch read guard orders the spawn against shutdown
            // publishing its stop signal, including at the end of backoff.
            next = {
                let shutdown = self.shutdown.borrow();
                if *shutdown {
                    return;
                }
                launch()
            };
            attempt = attempt.saturating_add(1);
        }
    }

    fn record_failure(&mut self, at: Instant, ready_since: Option<Instant>) -> Option<Duration> {
        let retry = self.budget.failure(at, ready_since);
        if retry.is_none() {
            self.state.send_replace(State::Failed);
            tracing::error!("desktop exhausted its rolling restart budget");
        }
        retry
    }

    async fn drive(
        &mut self,
        child: &mut DesktopProcess,
        attempt: u64,
        display: &str,
    ) -> (End, Option<Instant>) {
        if attempt != 0 {
            let start = Instant::now();
            let token = child.websocket_token().to_owned();
            let readiness = tokio::time::timeout(STARTUP_LIMIT, async {
                loop {
                    if authenticate(&token, display).await.is_ok() {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            });
            tokio::select! {
                biased;
                () = stopped(self.shutdown.clone()) => return (End::Shutdown, None),
                result = child.stopped() => return (observed(result), None),
                result = readiness => {
                    if result.is_err() {
                        return (End::StartupTimeout, None);
                    }
                }
            }
            tracing::info!(
                attempt,
                elapsed_ms = start.elapsed().as_millis(),
                "replacement desktop ready"
            );
        }
        let ready_since = Instant::now();
        let (generation, owner) = DesktopGeneration::supervised(child.websocket_token().into());
        {
            let shutdown = self.shutdown.borrow();
            if *shutdown {
                return (End::Shutdown, None);
            }
            self.state.send_replace(State::Ready(generation));
        }
        let result = tokio::select! {
            biased;
            () = stopped(self.shutdown.clone()) => End::Shutdown,
            result = child.stopped() => observed(result),
        };
        drop(owner);
        (result, Some(ready_since))
    }
}

fn observed(result: Result<(), String>) -> End {
    match result {
        Ok(()) => End::Exit,
        Err(error) => End::Uncertain(error),
    }
}

async fn stopped(mut shutdown: watch::Receiver<bool>) {
    let _ = shutdown.wait_for(|stopped| *stopped).await;
}

async fn wait_to_retry(shutdown: watch::Receiver<bool>, delay: Duration) -> bool {
    tokio::select! {
        biased;
        () = stopped(shutdown) => false,
        () = tokio::time::sleep(delay) => true,
    }
}

#[cfg(test)]
mod tests;
