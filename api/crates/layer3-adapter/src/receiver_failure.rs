//! Scoped observations from actual failed receiver negotiations. Never an ACK.
use std::{future::Future, sync::Mutex};

/// Reconstructs a challenge without retaining an endpoint or secret.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetirementReceiver {
    pub runtime: crate::RuntimeHandle,
    pub pod: crate::RetirementPod,
    pub owner: crate::owner_protocol::TransportOwner,
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

pub(crate) fn record(receiver: impl FnOnce() -> Option<RetirementReceiver>) {
    let _ = OBSERVATION.try_with(|slot| {
        *slot.lock().expect("receiver observation") = Some(ObservedFailure {
            receiver: receiver(),
        })
    });
}

/// A typed failure of the attempted control exchange; it never proves absence.
pub struct ObservedFailure {
    receiver: Option<RetirementReceiver>,
}
impl ObservedFailure {
    pub fn into_receiver(self) -> Option<RetirementReceiver> {
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
