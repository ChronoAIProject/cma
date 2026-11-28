use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::{DesktopGeneration, DesktopProcess, DesktopState, DesktopUnavailable};

pub(crate) const DESKTOP_PROBE_INTERVAL: Duration = Duration::from_millis(100);
const DESKTOP_EXIT_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LegacyState {
    Starting,
    Ready,
    Failed,
}

enum StartupOutcome {
    Ready,
    Exited,
    Timeout,
    Shutdown,
}

pub(crate) struct LegacyDesktop {
    token: String,
    watch_child: bool,
    state: watch::Sender<LegacyState>,
    shutdown: watch::Sender<bool>,
    process: Arc<Mutex<Option<DesktopProcess>>>,
    startup: Mutex<Option<JoinHandle<()>>>,
}

impl LegacyDesktop {
    pub(crate) fn new(process: DesktopProcess) -> Self {
        let token = process.websocket_token().to_owned();
        Self {
            token,
            watch_child: true,
            state: watch::Sender::new(LegacyState::Starting),
            shutdown: watch::Sender::new(false),
            process: Arc::new(Mutex::new(Some(process))),
            startup: Mutex::new(None),
        }
    }

    #[cfg(test)]
    pub(crate) fn probe_only(token: String) -> Self {
        Self {
            token,
            watch_child: false,
            state: watch::Sender::new(LegacyState::Starting),
            shutdown: watch::Sender::new(false),
            process: Arc::new(Mutex::new(None)),
            startup: Mutex::new(None),
        }
    }

    pub(super) fn status(&self) -> DesktopState {
        match *self.state.borrow() {
            LegacyState::Starting => DesktopState::Starting,
            LegacyState::Ready => DesktopState::Ready,
            LegacyState::Failed => DesktopState::Failed,
        }
    }

    pub(super) fn generation(&self) -> Result<DesktopGeneration, DesktopUnavailable> {
        match *self.state.borrow() {
            LegacyState::Ready => Ok(DesktopGeneration::legacy(self.token.clone())),
            LegacyState::Starting => Err(DesktopUnavailable { retryable: true }),
            LegacyState::Failed => Err(DesktopUnavailable { retryable: false }),
        }
    }

    pub(super) fn start_background_with_probe<Fut>(
        &self,
        budget: Duration,
        mut connect: impl FnMut() -> Fut + Send + 'static,
    ) where
        Fut: Future<Output = bool> + Send + 'static,
    {
        let mut startup = self
            .startup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if startup.is_some()
            || *self.shutdown.borrow()
            || !matches!(*self.state.borrow(), LegacyState::Starting)
        {
            return;
        }
        let state = self.state.clone();
        let shutdown = self.shutdown.subscribe();
        let process = Arc::clone(&self.process);
        let watch_child = self.watch_child;
        *startup = Some(tokio::spawn(async move {
            let started = Instant::now();
            let probe_process = Arc::clone(&process);
            let probe = async move {
                loop {
                    if child_exited(&probe_process, watch_child) {
                        return StartupOutcome::Exited;
                    }
                    if connect().await {
                        return if child_exited(&probe_process, watch_child) {
                            StartupOutcome::Exited
                        } else {
                            StartupOutcome::Ready
                        };
                    }
                    tokio::time::sleep(DESKTOP_PROBE_INTERVAL).await;
                }
            };
            let exit_watch = async {
                if !watch_child {
                    std::future::pending::<()>().await;
                }
                loop {
                    if child_exited(&process, true) {
                        return;
                    }
                    tokio::time::sleep(DESKTOP_EXIT_POLL_INTERVAL).await;
                }
            };
            let outcome = tokio::select! {
                biased;
                () = stopped(shutdown.clone()) => StartupOutcome::Shutdown,
                () = exit_watch => StartupOutcome::Exited,
                () = tokio::time::sleep(budget) => StartupOutcome::Timeout,
                outcome = probe => outcome,
            };
            match outcome {
                StartupOutcome::Ready => {
                    let stopped_flag = shutdown.borrow();
                    if *stopped_flag {
                        drop_process(&process);
                        return;
                    }
                    state.send_replace(LegacyState::Ready);
                    tracing::info!(
                        elapsed_ms = started.elapsed().as_millis(),
                        "initial desktop ready"
                    );
                }
                StartupOutcome::Exited => {
                    tracing::error!(
                        elapsed_ms = started.elapsed().as_millis(),
                        "initial desktop exited before readiness"
                    );
                    drop_process(&process);
                    state.send_replace(LegacyState::Failed);
                }
                StartupOutcome::Timeout => {
                    tracing::error!(
                        elapsed_ms = started.elapsed().as_millis(),
                        "initial desktop did not become ready"
                    );
                    drop_process(&process);
                    state.send_replace(LegacyState::Failed);
                }
                StartupOutcome::Shutdown => {
                    drop_process(&process);
                }
            }
        }));
    }

    #[cfg(test)]
    pub(super) async fn wait_initial_ready(&self) -> bool {
        let mut state = self.state.subscribe();
        let mut shutdown = self.shutdown.subscribe();
        tokio::select! {
            biased;
            _ = shutdown.wait_for(|stopped| *stopped) => false,
            result = state.wait_for(|state| !matches!(state, LegacyState::Starting)) => {
                result.is_ok_and(|state| matches!(*state, LegacyState::Ready))
            }
        }
    }

    pub(super) async fn shutdown(&self) {
        self.shutdown.send_replace(true);
        let task = self
            .startup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(task) = task {
            if let Err(error) = task.await {
                if !error.is_cancelled() {
                    tracing::error!(%error, "desktop startup task failed");
                }
            }
        }
        drop_process(&self.process);
    }
}

impl Drop for LegacyDesktop {
    fn drop(&mut self) {
        self.shutdown.send_replace(true);
        if let Some(task) = self
            .startup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
        drop_process(&self.process);
    }
}

fn child_exited(process: &Mutex<Option<DesktopProcess>>, watch_child: bool) -> bool {
    if !watch_child {
        return false;
    }
    process
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_none_or(DesktopProcess::has_exited_unreaped)
}

fn drop_process(process: &Mutex<Option<DesktopProcess>>) {
    let _ = process
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
}

async fn stopped(mut shutdown: watch::Receiver<bool>) {
    let _ = shutdown.wait_for(|stopped| *stopped).await;
}
