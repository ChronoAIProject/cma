use std::time::Duration;

use tokio::sync::{oneshot, watch, Mutex};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::restart_budget::RestartBudget;
use super::transport::authenticate;
use super::{DesktopGeneration, DesktopProcess, DesktopUnavailable};
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

    pub(crate) async fn ready(&self, startup_limit: Duration) -> bool {
        let mut owner = self.owner.lock().await;
        if !matches!(*owner, ProcessOwner::Starting(_)) {
            return self.generation().is_ok();
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
            startup_limit,
        };
        let (first_ready, confirmed) = oneshot::channel();
        *owner = ProcessOwner::Running(tokio::spawn(async move {
            driver
                .run(
                    *initial,
                    || DesktopProcess::start(&config),
                    Some(first_ready),
                )
                .await;
        }));
        confirmed.await.is_ok()
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
    startup_limit: Duration,
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
        mut first_ready: Option<oneshot::Sender<()>>,
    ) {
        let mut next = Ok(initial);
        let mut attempt = 0_u64;
        loop {
            let retry = match next {
                Ok(mut child) => {
                    let (end, ready_since) =
                        self.drive(&mut child, attempt, first_ready.take()).await;
                    let failed_at = Instant::now();
                    let delay = match &end {
                        End::Exit | End::StartupTimeout
                            if attempt == 0 && ready_since.is_none() =>
                        {
                            self.state.send_replace(State::Failed);
                            None
                        }
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
                            tracing::warn!(attempt, "desktop generation failed readiness")
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
        first_ready: Option<oneshot::Sender<()>>,
    ) -> (End, Option<Instant>) {
        let start = Instant::now();
        let token = child.websocket_token().to_owned();
        let (files, leader) = child.display_directory();
        let directory = files.path.clone();
        let token_map = files.token_map.clone();
        let mut backend = None;
        let deadline = start + self.startup_limit;
        let readiness = async {
            loop {
                if Instant::now() >= deadline {
                    return Err(End::StartupTimeout);
                }
                if let Ok(listener) =
                    authenticate(&token, &directory, leader, &token_map, &mut backend).await
                {
                    return if Instant::now() < deadline {
                        Ok(listener)
                    } else {
                        Err(End::StartupTimeout)
                    };
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        };
        let listener = tokio::select! {
            biased;
            () = stopped(self.shutdown.clone()) => Err(End::Shutdown),
            result = child.stopped() => Err(observed(result)),
            () = tokio::time::sleep_until(deadline) => Err(End::StartupTimeout),
            listener = readiness => listener,
        };
        // Retain the mapped descriptor even if readiness was cancelled.
        child.backend = backend;
        let listener = match listener {
            Ok(listener) => listener,
            Err(end) => return (end, None),
        };
        if attempt != 0 {
            tracing::info!(
                attempt,
                elapsed_ms = start.elapsed().as_millis(),
                "replacement desktop ready"
            );
        }
        let ready_since = Instant::now();
        let (generation, owner) = DesktopGeneration::supervised(token, listener);
        {
            let shutdown = self.shutdown.borrow();
            if *shutdown {
                return (End::Shutdown, None);
            }
            self.state.send_replace(State::Ready(generation));
            if let Some(confirmed) = first_ready {
                let _ = confirmed.send(());
            }
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
