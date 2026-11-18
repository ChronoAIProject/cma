//! Scoped observations from actual failed receiver negotiations. Never an ACK.
use std::{
    future::Future,
    sync::{Arc, Mutex},
};

/// Only the transport can construct a retry of an observed receiver failure.
/// It repeats the challenge, without confirmation, acquisition or lease renewal.
#[async_trait::async_trait]
pub trait ReceiverFailureProbe: Send + Sync {
    async fn still_unavailable(&self) -> bool;
}

tokio::task_local! {
    static OBSERVATION: Mutex<Option<ObservedFailure>>;
}

pub async fn observe<T>(future: impl Future<Output = T>) -> (T, Option<ObservedFailure>) {
    OBSERVATION
        .scope(Mutex::new(None), async move {
            let result = future.await;
            let observation =
                OBSERVATION.with(|slot| slot.lock().expect("receiver observation").take());
            (result, observation)
        })
        .await
}

pub(crate) fn record(probe: Arc<dyn ReceiverFailureProbe>) {
    let _ = OBSERVATION.try_with(|slot| {
        *slot.lock().expect("receiver observation") = Some(ObservedFailure {
            receiver: Some(probe),
        })
    });
}

/// A typed failure of the attempted control exchange; it never proves absence.
pub struct ObservedFailure {
    receiver: Option<Arc<dyn ReceiverFailureProbe>>,
}
impl ObservedFailure {
    pub fn into_receiver(self) -> Option<Arc<dyn ReceiverFailureProbe>> {
        self.receiver
    }
}

pub(crate) struct ReadAttempt {
    armed: bool,
}
impl ReadAttempt {
    pub(crate) fn new(read_only: bool) -> Self {
        Self { armed: read_only }
    }
    pub(crate) fn http_status(&self, status: http::StatusCode) {
        if self.armed && (status == http::StatusCode::NOT_FOUND || status.is_server_error()) {
            read_failed();
        }
    }
    pub(crate) fn finish<T>(&mut self, result: &crate::Result<T>) {
        if self.armed && matches!(result, Err(crate::AdapterError::Unavailable(_))) {
            read_failed();
        }
        self.armed = false;
    }
}
impl Drop for ReadAttempt {
    fn drop(&mut self) {
        // Cancellation of a polled status exchange is observed inside the
        // caller's bounded read attempt. Cancelling the caller cannot publish it.
        if self.armed {
            read_failed();
        }
    }
}
fn read_failed() {
    let _ = OBSERVATION.try_with(|slot| {
        *slot.lock().expect("control observation") = Some(ObservedFailure { receiver: None })
    });
}
