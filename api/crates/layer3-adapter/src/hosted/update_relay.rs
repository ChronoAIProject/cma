use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tokio::sync::mpsc;

use crate::ingress_budget::{
    AllocationPlan, IngressClass, IngressOwnershipPolicy, ReservationBundle,
};
use crate::ConversationUpdate;

const RELAY_CAPACITY: usize = 32;

/// Ordered delivery; bounded mode relies on the reader's concurrent ACK pump.
#[derive(Clone)]
pub(super) struct UpdateRelay {
    ingress: Ingress,
}

#[derive(Clone)]
enum Ingress {
    Legacy(mpsc::UnboundedSender<ConversationUpdate>),
    Bounded(mpsc::Sender<ConversationUpdate>),
    Admitted {
        sender: mpsc::Sender<ConversationUpdate>,
        queue: Arc<RelayQueue>,
    },
    Capture(Arc<CapturedUpdate>),
}

struct RelayQueue {
    enqueue: Mutex<()>,
    _storage: ReservationBundle,
}

pub(super) struct CapturedUpdate {
    value: Mutex<Option<ConversationUpdate>>,
    overflow: AtomicBool,
    _storage: ReservationBundle,
}

impl CapturedUpdate {
    pub(super) fn take(&self) -> crate::Result<Option<ConversationUpdate>> {
        if self.overflow.load(Ordering::SeqCst) {
            return Err(crate::AdapterError::Rejected(
                "one runtime frame produced multiple domain updates".into(),
            ));
        }
        self.value
            .lock()
            .map(|mut value| value.take())
            .map_err(|_| {
                crate::AdapterError::Unavailable("runtime output capture is poisoned".into())
            })
    }
}

impl UpdateRelay {
    pub(super) fn capture_admitted(
        policy: &IngressOwnershipPolicy,
    ) -> crate::Result<(Self, Arc<CapturedUpdate>)> {
        let plan = AllocationPlan::new(
            "single frame capture",
            &[
                size_of::<CapturedUpdate>() + 2 * size_of::<usize>(),
                64, // Darwin's lazily initialized pthread mutex backing storage.
            ],
        )?;
        let storage = policy.reserve(IngressClass::Data, plan)?;
        let capture = Arc::new(CapturedUpdate {
            value: Mutex::new(None),
            overflow: AtomicBool::new(false),
            _storage: storage,
        });
        Ok((
            Self {
                ingress: Ingress::Capture(capture.clone()),
            },
            capture,
        ))
    }

    /// Collect the domain effect of one frame without publishing before its receipt.
    pub(super) fn capture() -> (Self, mpsc::UnboundedReceiver<ConversationUpdate>) {
        let (ingress, receiver) = mpsc::unbounded_channel();
        (
            Self {
                ingress: Ingress::Legacy(ingress),
            },
            receiver,
        )
    }

    pub(super) fn bounded(&self) -> bool {
        matches!(self.ingress, Ingress::Bounded(_) | Ingress::Admitted { .. })
    }

    pub(super) async fn closed(&self) {
        match &self.ingress {
            Ingress::Legacy(ingress) => ingress.closed().await,
            Ingress::Bounded(ingress) => ingress.closed().await,
            Ingress::Admitted { sender, .. } => sender.closed().await,
            Ingress::Capture(_) => std::future::pending().await,
        }
    }

    pub(super) async fn send(&self, update: ConversationUpdate) {
        let sent = match &self.ingress {
            Ingress::Legacy(ingress) => ingress.send(update),
            Ingress::Bounded(ingress) => ingress.send(update).await,
            Ingress::Admitted { sender, queue } => match sender.reserve().await {
                Ok(slot) => match queue.enqueue.lock() {
                    Ok(_enqueue) => {
                        slot.send(update);
                        Ok(())
                    }
                    Err(_) => Err(mpsc::error::SendError(update)),
                },
                Err(_) => Err(mpsc::error::SendError(update)),
            },
            Ingress::Capture(capture) => match capture.value.lock() {
                Ok(mut value) if value.is_none() => {
                    *value = Some(update);
                    Ok(())
                }
                _ => {
                    capture.overflow.store(true, Ordering::SeqCst);
                    Err(mpsc::error::SendError(update))
                }
            },
        };
        if sent.is_err() {
            tracing::debug!("conversation update relay has ended");
        }
    }
}

#[cfg(test)]
#[path = "update_relay_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "update_relay_ingress_tests.rs"]
mod ingress_tests;

pub(super) fn spawn_admitted(
    output: mpsc::Sender<ConversationUpdate>,
    policy: &IngressOwnershipPolicy,
) -> crate::Result<(UpdateRelay, tokio::task::JoinHandle<()>)> {
    let plan = AllocationPlan::new(
        "bounded update relay",
        &[
            crate::ingress_budget::sizing::channel_bytes::<ConversationUpdate>(RELAY_CAPACITY, 1)?,
            size_of::<RelayQueue>() + 2 * size_of::<usize>() + 64,
        ],
    )?;
    let storage = policy.reserve(IngressClass::Data, plan)?;
    let (sender, receiver) = mpsc::channel(RELAY_CAPACITY);
    let queue = Arc::new(RelayQueue {
        enqueue: Mutex::new(()),
        _storage: storage,
    });
    struct Receiver {
        receiver: mpsc::Receiver<ConversationUpdate>,
        _queue: Arc<RelayQueue>,
    }
    let mut queued = Receiver {
        receiver,
        _queue: queue.clone(),
    };
    let task = spawn_owned(async move {
        loop {
            let update = tokio::select! {
                _ = output.closed() => break,
                update = queued.receiver.recv() => match update {
                    Some(update) => update,
                    None => break,
                },
            };
            if output.send(update).await.is_err() {
                break;
            }
        }
        drop(queued);
    });
    Ok((
        UpdateRelay {
            ingress: Ingress::Admitted { sender, queue },
        },
        task,
    ))
}

pub(super) fn spawn(
    output: mpsc::Sender<ConversationUpdate>,
    bounded: bool,
) -> (UpdateRelay, tokio::task::JoinHandle<()>) {
    if bounded {
        let (ingress, mut queued) = mpsc::channel(RELAY_CAPACITY);
        let task = spawn_owned(async move {
            loop {
                let update = tokio::select! {
                    _ = output.closed() => break,
                    update = queued.recv() => match update {
                        Some(update) => update,
                        None => break,
                    },
                };
                if output.send(update).await.is_err() {
                    break;
                }
            }
        });
        return (
            UpdateRelay {
                ingress: Ingress::Bounded(ingress),
            },
            task,
        );
    }
    let (ingress, mut queued) = mpsc::unbounded_channel();
    let task = spawn_owned(async move {
        while let Some(update) = queued.recv().await {
            if output.send(update).await.is_err() {
                break;
            }
        }
    });
    (
        UpdateRelay {
            ingress: Ingress::Legacy(ingress),
        },
        task,
    )
}

fn spawn_owned(
    relay: impl std::future::Future<Output = ()> + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    // Capture before spawning: task-local authority is not inherited by Tokio.
    let authority = crate::owner_protocol::current_authority();
    tokio::spawn(async move {
        match authority {
            Some(authority) => tokio::select! { biased;
                _ = authority.revoked() => (),
                _ = relay => (),
            },
            None => relay.await,
        }
    })
}
