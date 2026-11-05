//! Shared admission for optional Mongo work. Foreground work never waits here.
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use axum::{extract::Request, middleware::Next, response::Response};
use layer2_core::sandbox::store::SandboxStoreError;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

mod work;
pub(crate) use work::BackgroundWork;

const BACKGROUND_CONCURRENCY: usize = 2;
static SHARED: LazyLock<Arc<BackgroundBudget>> = LazyLock::new(BackgroundBudget::new);
static SANDBOX_RECONCILIATION: LazyLock<BackgroundWork> = LazyLock::new(|| {
    BackgroundWork::new(
        shared(),
        "sandbox_reconciliation",
        Duration::from_secs(30),
        Duration::from_secs(15),
    )
});

#[derive(Default)]
struct Priority {
    foreground: usize,
}

pub(crate) struct BackgroundBudget {
    slots: Arc<Semaphore>,
    priority: Mutex<Priority>,
}

impl BackgroundBudget {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            slots: Arc::new(Semaphore::new(BACKGROUND_CONCURRENCY)),
            priority: Mutex::new(Priority::default()),
        })
    }

    fn try_enter(&self) -> Option<OwnedSemaphorePermit> {
        let priority = self.priority.lock().unwrap();
        // While foreground work runs, admit at most one new background chain.
        // Existing work is not preempted: its durable effect may be uncertain.
        // Retaining one slot for background progress avoids starving recovery.
        if priority.foreground > 0 && self.slots.available_permits() <= 1 {
            return None;
        }
        self.slots.clone().try_acquire_owned().ok()
    }

    pub(crate) fn foreground(self: &Arc<Self>) -> Foreground {
        self.priority.lock().unwrap().foreground += 1;
        Foreground(self.clone())
    }
}

pub(crate) struct Foreground(Arc<BackgroundBudget>);
impl Drop for Foreground {
    fn drop(&mut self) {
        self.0.priority.lock().unwrap().foreground -= 1;
    }
}

pub(crate) fn shared() -> Arc<BackgroundBudget> {
    SHARED.clone()
}

pub(crate) async fn interactive_request(request: Request, next: Next) -> Response {
    let _priority = shared().foreground();
    // Release after response construction, not the lifetime of an SSE body.
    next.run(request).await
}

pub(crate) async fn sandbox_reconciliation<F>(
    operation: impl FnOnce() -> F,
) -> Result<(), SandboxStoreError>
where
    F: std::future::Future<Output = Result<(), SandboxStoreError>>,
{
    SANDBOX_RECONCILIATION.run(operation).await.map(|_| ())
}

#[cfg(test)]
mod tests;
