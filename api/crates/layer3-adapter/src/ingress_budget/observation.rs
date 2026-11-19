//! Delivery watches preserve storage custody through the input wait.

use tokio::{sync::watch, time::Instant};

use super::AdmittedWatchReceiver;

#[derive(Clone, Debug, Default)]
pub struct DeliveryObservation {
    pub(crate) progress: Option<DeliveryWatchReceiver<u64>>,
    pub(crate) history: Option<DeliveryWatchReceiver<Option<Instant>>>,
}

impl DeliveryObservation {
    pub fn admitted(
        progress: Option<AdmittedWatchReceiver<u64>>,
        history: Option<AdmittedWatchReceiver<Option<Instant>>>,
    ) -> Self {
        Self {
            progress: progress.map(DeliveryWatchReceiver::Admitted),
            history: history.map(DeliveryWatchReceiver::Admitted),
        }
    }

    pub fn admitted_progress(&self) -> Option<AdmittedWatchReceiver<u64>> {
        self.progress.as_ref()?.admitted()
    }

    pub fn admitted_history(&self) -> Option<AdmittedWatchReceiver<Option<Instant>>> {
        self.history.as_ref()?.admitted()
    }

    pub(crate) fn legacy(
        progress: Option<watch::Receiver<u64>>,
        history: Option<watch::Receiver<Option<Instant>>>,
    ) -> Self {
        Self {
            progress: progress.map(DeliveryWatchReceiver::Legacy),
            history: history.map(DeliveryWatchReceiver::Legacy),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum DeliveryWatchReceiver<T: Copy> {
    Legacy(watch::Receiver<T>),
    Admitted(AdmittedWatchReceiver<T>),
}

impl<T: Copy> DeliveryWatchReceiver<T> {
    pub(crate) fn latest(&self) -> T {
        match self {
            Self::Legacy(receiver) => *receiver.borrow(),
            Self::Admitted(receiver) => receiver.latest(),
        }
    }

    pub(crate) fn take_latest(&mut self) -> T {
        match self {
            Self::Legacy(receiver) => *receiver.borrow_and_update(),
            Self::Admitted(receiver) => receiver.take_latest(),
        }
    }

    pub(crate) async fn changed(&mut self) -> Result<(), watch::error::RecvError> {
        match self {
            Self::Legacy(receiver) => receiver.changed().await,
            Self::Admitted(receiver) => receiver.changed().await,
        }
    }

    fn admitted(&self) -> Option<AdmittedWatchReceiver<T>> {
        match self {
            Self::Legacy(_) => None,
            Self::Admitted(receiver) => Some(receiver.clone()),
        }
    }
}

#[cfg(test)]
#[path = "observation_tests.rs"]
mod tests;
