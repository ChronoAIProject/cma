use crate::ingress_budget::{
    observation::DeliveryWatchReceiver, AdmissionRefusal, AdmittedWatchSender, IngressClass,
    IngressOwnershipPolicy,
};
use tokio::sync::watch;

#[derive(Clone)]
pub(in crate::hosted) enum ProgressSender<T: Copy> {
    Legacy(watch::Sender<T>),
    Admitted(AdmittedWatchSender<T>),
}

impl<T: Copy> ProgressSender<T> {
    pub(in crate::hosted) fn new(
        initial: T,
        policy: Option<&IngressOwnershipPolicy>,
    ) -> Result<Self, AdmissionRefusal> {
        match policy {
            Some(policy) => AdmittedWatchSender::channel(policy, IngressClass::Data, initial)
                .map(|(sender, _)| Self::Admitted(sender)),
            None => Ok(Self::Legacy(watch::channel(initial).0)),
        }
    }

    pub(in crate::hosted) fn send_replace(&self, value: T) {
        match self {
            Self::Legacy(sender) => {
                sender.send_replace(value);
            }
            Self::Admitted(sender) => {
                sender.send_replace(value);
            }
        }
    }

    pub(in crate::hosted) fn send_if_modified(&self, update: impl FnOnce(&mut T) -> bool) {
        match self {
            Self::Legacy(sender) => {
                sender.send_if_modified(update);
            }
            Self::Admitted(sender) => {
                sender.send_if_modified(update);
            }
        }
    }

    pub(in crate::hosted) fn observe(&self) -> DeliveryWatchReceiver<T> {
        match self {
            Self::Legacy(sender) => DeliveryWatchReceiver::Legacy(sender.subscribe()),
            Self::Admitted(sender) => DeliveryWatchReceiver::Admitted(sender.subscribe()),
        }
    }

    pub(in crate::hosted) fn legacy_receiver(&self) -> Option<watch::Receiver<T>> {
        match self {
            Self::Legacy(sender) => Some(sender.subscribe()),
            Self::Admitted(_) => None,
        }
    }
}
