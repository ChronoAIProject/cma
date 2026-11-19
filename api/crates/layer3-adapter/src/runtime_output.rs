//! Private runtime delivery receipts. These are not public API response fields.

use crate::ingress_budget::{
    custody::SharedCustody,
    sizing::{channel_bytes_checked, shared_owner_bytes},
    AllocationPlan, IngressClass, IngressOwnershipPolicy, ReservationBundle, SharedOwned,
};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tokio::sync::mpsc;

#[path = "runtime_output_ack_error.rs"]
mod ack_error;
pub use ack_error::AdmittedAckError;

/// The effect and original receipt remain borrowed from the same delivery owner.
#[derive(Debug, Clone, Copy)]
pub struct RuntimeOutputDelivery<'a> {
    pub receipt: Option<&'a RuntimeOutputReceipt>,
    pub effect: Option<&'a crate::ConversationUpdate>,
}

/// An immutable native output position, scoped again by the durable Agent generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOutputSource {
    pub stream_id: String,
    pub cursor: u64,
    pub native_conversation_id: String,
    pub payload_sha256: String,
}

/// Only the durable consumer may release a retained runtime frame.
#[derive(Clone)]
pub struct RuntimeOutputReceipt {
    pub source: RuntimeOutputSource,
    acknowledgements: Acknowledgements,
}

#[derive(Clone)]
enum Acknowledgements {
    Legacy(mpsc::Sender<RuntimeOutputSource>),
    Admitted(AdmittedAckSender),
}

#[derive(Clone)]
pub struct AdmittedAckSender {
    sender: mpsc::Sender<SharedOwned<RuntimeOutputSource>>,
    queue: SharedCustody<AckQueue>,
    policy: IngressOwnershipPolicy,
}

struct AckQueue {
    // Serializing only the synchronous enqueue bounds speculative Tokio blocks.
    enqueue: Mutex<()>,
    _storage: ReservationBundle,
}

pub struct AdmittedAckReceiver {
    receiver: mpsc::Receiver<SharedOwned<RuntimeOutputSource>>,
    _queue: SharedCustody<AckQueue>,
}

impl AdmittedAckReceiver {
    pub async fn recv(&mut self) -> Option<SharedOwned<RuntimeOutputSource>> {
        self.receiver.recv().await
    }
}

impl AdmittedAckSender {
    fn send(&self, source: &RuntimeOutputSource) -> crate::Result<()> {
        self.send_admitted(source).map_err(AdmittedAckError::legacy)
    }

    fn send_admitted(&self, source: &RuntimeOutputSource) -> Result<(), AdmittedAckError> {
        if source.stream_id.len() > 36
            || source.native_conversation_id.len() > 1024
            || source.payload_sha256.len() > 64
        {
            return Err(AdmittedAckError::IdentityTooLarge);
        }
        let _enqueue = self
            .queue
            .enqueue
            .lock()
            .map_err(|_| AdmittedAckError::Poisoned)?;
        let slot = self.sender.try_reserve().map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => AdmittedAckError::Full,
            mpsc::error::TrySendError::Closed(_) => AdmittedAckError::Closed,
        })?;
        let plan = AllocationPlan::checked(
            "acknowledgement identity",
            &[
                source.stream_id.len(),
                source.native_conversation_id.len(),
                source.payload_sha256.len(),
                shared_owner_bytes::<RuntimeOutputSource>(),
            ],
        )?;
        let storage = self.policy.admit(IngressClass::Control, plan)?;
        slot.send(storage.initialize(|| source.clone()));
        Ok(())
    }
}

impl RuntimeOutputReceipt {
    pub(crate) fn control_bound(capacity: usize) -> crate::Result<AllocationPlan> {
        let source = AllocationPlan::new(
            "maximum acknowledgement identity",
            &[36, 1024, 64, shared_owner_bytes::<RuntimeOutputSource>()],
        )?
        .requested_bytes();
        let copies = capacity
            .checked_add(1)
            .and_then(|n| n.checked_mul(source))
            .ok_or_else(|| {
                crate::AdapterError::Rejected("acknowledgement layout overflow".into())
            })?;
        AllocationPlan::new(
            "attachment acknowledgement control",
            &[
                Self::queue_plan(capacity)?.requested_bytes(),
                copies,
                25 + shared_owner_bytes::<tokio_tungstenite::tungstenite::Message>(),
            ],
        )
    }

    fn queue_plan(capacity: usize) -> crate::Result<AllocationPlan> {
        Self::queue_plan_checked(capacity).map_err(Into::into)
    }

    fn queue_plan_checked(
        capacity: usize,
    ) -> Result<AllocationPlan, crate::ingress_budget::AdmissionRefusal> {
        AllocationPlan::checked(
            "acknowledgement queue",
            &[
                channel_bytes_checked::<SharedOwned<RuntimeOutputSource>>(capacity, 1)?,
                size_of::<AckQueue>() + 2 * size_of::<usize>(),
                // Darwin backing for the enqueue lock and the channel's
                // bounded semaphore wait-list lock, both initialized below.
                2 * 64,
            ],
        )
    }

    pub fn new(
        source: RuntimeOutputSource,
        acknowledgements: mpsc::Sender<RuntimeOutputSource>,
    ) -> Self {
        Self {
            source,
            acknowledgements: Acknowledgements::Legacy(acknowledgements),
        }
    }

    pub fn new_admitted(source: RuntimeOutputSource, acknowledgements: AdmittedAckSender) -> Self {
        Self {
            source,
            acknowledgements: Acknowledgements::Admitted(acknowledgements),
        }
    }

    pub fn admitted_channel(
        policy: IngressOwnershipPolicy,
        capacity: usize,
    ) -> crate::Result<(AdmittedAckSender, AdmittedAckReceiver)> {
        Self::admitted_channel_checked(policy, capacity).map_err(AdmittedAckError::legacy)
    }

    /// Refuse before allocating the channel or its diagnostics. The legacy
    /// constructor preserves its existing AdapterError interface.
    pub fn admitted_channel_checked(
        policy: IngressOwnershipPolicy,
        capacity: usize,
    ) -> Result<(AdmittedAckSender, AdmittedAckReceiver), AdmittedAckError> {
        if capacity == 0 {
            return Err(AdmittedAckError::InvalidCapacity);
        }
        let plan = Self::queue_plan_checked(capacity)?;
        let storage = policy.admit(IngressClass::Control, plan)?;
        let (sender, receiver) = mpsc::channel(capacity);
        // Returning a speculative slot on refusal must not be the first use
        // that allocates the channel semaphore's persistent mutex backing.
        drop(
            sender
                .try_reserve()
                .expect("new acknowledgement channel has capacity"),
        );
        let queue = SharedCustody::new(AckQueue {
            enqueue: Mutex::new(()),
            _storage: storage,
        });
        drop(
            queue
                .enqueue
                .lock()
                .expect("new acknowledgement mutex is uncontended"),
        );
        Ok((
            AdmittedAckSender {
                sender,
                queue: queue.clone(),
                policy,
            },
            AdmittedAckReceiver {
                receiver,
                _queue: queue,
            },
        ))
    }

    /// Call only after durable consumption, as with acknowledge(). Success
    /// enqueues the original position; it does not prove delivery to the peer.
    /// Every refusal is allocation-free and leaves the original receipt intact.
    pub fn acknowledge_admitted(&self) -> Result<(), AdmittedAckError> {
        match &self.acknowledgements {
            Acknowledgements::Admitted(sender) => sender.send_admitted(&self.source),
            Acknowledgements::Legacy(_) => Err(AdmittedAckError::OwnershipRequired),
        }
    }

    pub async fn acknowledge(&self) -> crate::Result<()> {
        match &self.acknowledgements {
            Acknowledgements::Legacy(sender) => {
                sender.send(self.source.clone()).await.map_err(|_| {
                    crate::AdapterError::Unavailable("runtime acknowledgement link ended".into())
                })
            }
            Acknowledgements::Admitted(sender) => sender.send(&self.source),
        }
    }
}

impl std::fmt::Debug for RuntimeOutputReceipt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("RuntimeOutputReceipt")
            .field(&self.source)
            .finish()
    }
}

#[cfg(test)]
#[path = "runtime_output_ownership_tests.rs"]
mod runtime_output_ownership_tests;

#[cfg(test)]
#[path = "runtime_output_ack_error_tests.rs"]
mod ack_error_tests;
