//! Shared admission for optional Mongo work. Foreground work never waits here.
use std::sync::{Arc, LazyLock, Mutex};

use axum::{extract::Request, middleware::Next, response::Response};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

mod work;
pub(crate) use work::BackgroundWork;
mod reservation;
use reservation::Reservation;

const BACKGROUND_CONCURRENCY: usize = 2;
static SHARED: LazyLock<Arc<BackgroundBudget>> = LazyLock::new(BackgroundBudget::new);

#[derive(Default)]
struct Priority {
    foreground: usize,
    reserved: usize,
}

pub(crate) struct BackgroundBudget {
    slots: Arc<Semaphore>,
    priority: Mutex<Priority>,
    capacity: usize,
    _reservation: Option<Reservation>,
}

impl BackgroundBudget {
    pub(crate) fn new() -> Arc<Self> {
        Self::with_concurrency(BACKGROUND_CONCURRENCY)
    }

    pub(crate) fn with_concurrency(capacity: usize) -> Arc<Self> {
        assert!(capacity > 0, "background work needs progress capacity");
        Arc::new(Self {
            slots: Arc::new(Semaphore::new(capacity)),
            priority: Mutex::new(Priority::default()),
            capacity,
            _reservation: None,
        })
    }

    fn try_enter(&self) -> Option<OwnedSemaphorePermit> {
        let priority = self.priority.lock().unwrap();
        // While foreground work runs, admit at most one new background chain.
        // Existing work is not preempted: its durable effect may be uncertain.
        // A reserved lane owns its capacity separately; count only unreserved
        // slots here so foreground traffic still permits one shared chain.
        let available_capacity = self.capacity - priority.reserved;
        if priority.foreground > 0 && self.slots.available_permits() < available_capacity {
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

#[cfg(test)]
mod reservation_tests;
#[cfg(test)]
mod tests;
