//! Private runtime delivery receipts. These are not public API response fields.

use crate::ingress_budget::{
    custody::SharedCustody,
    sizing::{channel_bytes, shared_owner_bytes},
    AllocationPlan, IngressClass, IngressOwnershipPolicy, ReservationBundle, SharedOwned,
};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tokio::sync::mpsc;

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
        if source.stream_id.len() > 36
            || source.native_conversation_id.len() > 1024
            || source.payload_sha256.len() > 64
        {
            return Err(crate::AdapterError::Rejected(
                "runtime acknowledgement identity exceeds its bound".into(),
            ));
        }
        let _enqueue = self.queue.enqueue.lock().map_err(|_| {
            crate::AdapterError::Unavailable("runtime acknowledgement queue is poisoned".into())
        })?;
        let slot = self.sender.try_reserve().map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => {
                crate::AdapterError::AtCapacity("runtime acknowledgement queue is full".into())
            }
            mpsc::error::TrySendError::Closed(_) => {
                crate::AdapterError::Unavailable("runtime acknowledgement link ended".into())
            }
        })?;
        let plan = AllocationPlan::new(
            "acknowledgement identity",
            &[
                source.stream_id.len(),
                source.native_conversation_id.len(),
                source.payload_sha256.len(),
                shared_owner_bytes::<RuntimeOutputSource>(),
            ],
        )?;
        let storage = self.policy.reserve(IngressClass::Control, plan)?;
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
        AllocationPlan::new(
            "acknowledgement queue",
            &[
                channel_bytes::<SharedOwned<RuntimeOutputSource>>(capacity, 1)?,
                size_of::<AckQueue>() + 2 * size_of::<usize>(),
                64, // Darwin's lazily initialized pthread mutex backing storage.
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
        if capacity == 0 {
            return Err(crate::AdapterError::Rejected(
                "acknowledgement capacity must be positive".into(),
            ));
        }
        let plan = Self::queue_plan(capacity)?;
        let storage = policy.reserve(IngressClass::Control, plan)?;
        let (sender, receiver) = mpsc::channel(capacity);
        let queue = SharedCustody::new(AckQueue {
            enqueue: Mutex::new(()),
            _storage: storage,
        });
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
