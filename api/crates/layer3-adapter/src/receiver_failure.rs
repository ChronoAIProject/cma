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

pub(crate) fn record(probe: impl FnOnce() -> Arc<dyn ReceiverFailureProbe>) {
    let _ = OBSERVATION.try_with(|slot| {
        *slot.lock().expect("receiver observation") = Some(ObservedFailure {
            receiver: Some(probe()),
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

pub(crate) fn unavailable_status(status: http::StatusCode) -> bool {
    status == http::StatusCode::NOT_FOUND || status.is_server_error()
}

pub(crate) fn record_status(status: http::StatusCode) {
    if unavailable_status(status) {
        let _ = OBSERVATION.try_with(|slot| {
            *slot.lock().expect("control observation") = Some(ObservedFailure { receiver: None })
        });
    }
}
