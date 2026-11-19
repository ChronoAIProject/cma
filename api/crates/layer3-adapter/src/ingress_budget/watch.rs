//! Fixed-value watches whose backing storage follows every sender and receiver.

use std::sync::RwLock;
use tokio::sync::{watch, Notify};

use super::{
    AdmissionRefusal, AllocationPlan, IngressClass, IngressOwnershipPolicy, IngressWorkScope,
    ReservationBundle,
};

#[derive(Debug)]
pub struct AdmittedWatchSender<T: Copy> {
    sender: watch::Sender<T>,
    storage: IngressWorkScope,
}

#[derive(Debug)]
pub struct AdmittedWatchReceiver<T: Copy> {
    receiver: watch::Receiver<T>,
    storage: IngressWorkScope,
}

impl<T: Copy> AdmittedWatchSender<T> {
    pub fn allocation_plan() -> Result<AllocationPlan, AdmissionRefusal> {
        if !cfg!(any(target_os = "linux", target_vendor = "apple")) {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "unsupported watch platform layout",
            });
        }
        // Tokio 1.53.3 Shared<T>: RwLock<T>, state, two reference counters,
        // eight receiver Notifies and one sender Notify. Include BigNotify's
        // optional index, the Arc counters, per-field and tail/header padding.
        // Rust 1.96.1 Apple Notify mutexes lazily box pthread_mutex_t. Initialize
        // them before publishing handles so concurrent first use cannot race to
        // allocate duplicate boxes. Linux's futex locks need no backing boxes.
        #[cfg(target_vendor = "apple")]
        let lock_backing = 9 * size_of::<libc::pthread_mutex_t>();
        #[cfg(not(target_vendor = "apple"))]
        let lock_backing = 0;
        AllocationPlan::checked(
            "watch backing storage",
            &[
                size_of::<RwLock<T>>(),
                align_of::<RwLock<T>>() - 1,
                9 * (size_of::<Notify>() + align_of::<Notify>() - 1),
                6 * (size_of::<usize>() + align_of::<usize>() - 1),
                align_of::<RwLock<T>>() + align_of::<Notify>(),
                lock_backing,
                size_of::<ReservationBundle>()
                    + 2 * size_of::<usize>()
                    + align_of::<ReservationBundle>(),
            ],
        )
    }

    pub fn channel(
        policy: &IngressOwnershipPolicy,
        class: IngressClass,
        initial: T,
    ) -> Result<(Self, AdmittedWatchReceiver<T>), AdmissionRefusal> {
        let storage = IngressWorkScope::new(policy.admit(class, Self::allocation_plan()?)?);
        let (sender, receiver) = watch::channel(initial);
        sender.send_replace(initial);
        drop(receiver);
        // A fresh subscription observes the initialized version as already seen.
        let receiver = sender.subscribe();
        Ok((
            Self {
                sender,
                storage: storage.clone(),
            },
            AdmittedWatchReceiver { receiver, storage },
        ))
    }

    pub fn subscribe(&self) -> AdmittedWatchReceiver<T> {
        AdmittedWatchReceiver {
            receiver: self.sender.subscribe(),
            storage: self.storage.clone(),
        }
    }

    pub fn latest(&self) -> T {
        *self.sender.borrow()
    }

    pub fn send_replace(&self, value: T) -> T {
        self.sender.send_replace(value)
    }

    pub fn send_if_modified(&self, update: impl FnOnce(&mut T) -> bool) -> bool {
        self.sender.send_if_modified(update)
    }

    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }

    pub async fn closed(&self) {
        self.sender.closed().await;
    }

    pub fn requested_bytes(&self) -> usize {
        self.storage.requested_bytes()
    }
}

impl<T: Copy> Clone for AdmittedWatchSender<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            storage: self.storage.clone(),
        }
    }
}

impl<T: Copy> AdmittedWatchReceiver<T> {
    pub fn belongs_to(&self, policy: &IngressOwnershipPolicy) -> bool {
        self.storage.belongs_to(policy)
    }

    pub fn observe_current(&self) -> Self {
        let mut receiver = self.clone();
        receiver.take_latest();
        receiver
    }

    pub fn latest(&self) -> T {
        *self.receiver.borrow()
    }

    pub fn take_latest(&mut self) -> T {
        *self.receiver.borrow_and_update()
    }

    pub fn has_changed(&self) -> Result<bool, watch::error::RecvError> {
        self.receiver.has_changed()
    }

    pub async fn changed(&mut self) -> Result<(), watch::error::RecvError> {
        self.receiver.changed().await
    }

    pub fn requested_bytes(&self) -> usize {
        self.storage.requested_bytes()
    }
}

impl<T: Copy> Clone for AdmittedWatchReceiver<T> {
    fn clone(&self) -> Self {
        Self {
            receiver: self.receiver.clone(),
            storage: self.storage.clone(),
        }
    }
}

#[cfg(test)]
#[path = "watch_tests.rs"]
mod tests;
