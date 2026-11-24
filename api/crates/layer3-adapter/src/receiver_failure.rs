//! Scoped observations from actual failed receiver negotiations. Never an ACK.
use std::{
    future::Future,
    sync::{Arc, Mutex, OnceLock},
};

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
    static CAPTURE: Arc<OnceLock<EligibleReceiverFailure>>;
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

/// A single attempt's observation, retained by the caller outside the scoped future.
pub struct ReceiverFailureCapture(Arc<OnceLock<EligibleReceiverFailure>>);

impl ReceiverFailureCapture {
    pub fn sealed(&self) -> Option<&EligibleReceiverFailure> {
        self.0.get()
    }
}

pub fn capture<F: Future>(attempt: F) -> (ReceiverFailureCapture, impl Future<Output = F::Output>) {
    let slot = Arc::new(OnceLock::new());
    (
        ReceiverFailureCapture(slot.clone()),
        CAPTURE.scope(slot, attempt),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChallengeRefusal {
    NotFound,
    ServerError,
}

impl ChallengeRefusal {
    pub(crate) fn from_status(status: http::StatusCode) -> Option<Self> {
        if status == http::StatusCode::NOT_FOUND {
            Some(Self::NotFound)
        } else if status.is_server_error() {
            Some(Self::ServerError)
        } else {
            None
        }
    }
}

/// Sealed only by an owned challenge exchange with an exact native receiver.
pub struct EligibleReceiverFailure {
    receiver: RetirementReceiver,
    refusal: ChallengeRefusal,
}

impl EligibleReceiverFailure {
    pub fn receiver(&self) -> &RetirementReceiver {
        &self.receiver
    }

    pub fn refusal(&self) -> ChallengeRefusal {
        self.refusal
    }
}

pub(crate) fn record(
    refusal: ChallengeRefusal,
    receiver: impl FnOnce() -> Option<RetirementReceiver>,
) {
    let capture = CAPTURE.try_with(Arc::clone).ok();
    if capture.is_none() && OBSERVATION.try_with(|_| ()).is_err() {
        return;
    }
    let receiver = receiver();
    if let (Some(capture), Some(receiver)) = (capture, receiver.as_ref()) {
        if crate::owner_protocol::current_authority()
            .is_some_and(|authority| authority.is_current() && authority.owner() == receiver.owner)
        {
            // The first eligible exchange owns this attempt's single slot.
            let _ = capture.set(EligibleReceiverFailure {
                receiver: receiver.clone(),
                refusal,
            });
        }
    }
    let _ = OBSERVATION.try_with(|slot| {
        *slot.lock().expect("receiver observation") = Some(ObservedFailure { receiver });
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
    ChallengeRefusal::from_status(status).is_some()
}

pub(crate) fn record_status(status: http::StatusCode) {
    if unavailable_status(status) {
        let _ = OBSERVATION.try_with(|slot| {
            *slot.lock().expect("control observation") = Some(ObservedFailure { receiver: None })
        });
    }
}
