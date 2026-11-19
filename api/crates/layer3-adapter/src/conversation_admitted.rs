use super::*;
use crate::ingress_budget::{
    sizing, AdmissionRefusal, AdmittedError, AdmittedWatchReceiver, AdmittedWatchSender,
    AllocationPlan, IngressClass, IngressOwnershipPolicy, IngressWorkScope, OwnedWork, SharedOwned,
};

#[derive(Clone, Debug)]
pub enum AdmittedConversationSendOutcome {
    PreWriteFailure(AdmittedError),
    WriteConfirmed,
    Ambiguous(AdmittedError),
}

#[derive(Clone)]
pub struct AdmittedConversationInputSender {
    inner: mpsc::Sender<AdmittedInput>,
    cancelled: AdmittedWatchSender<bool>,
    progress: Option<AdmittedWatchReceiver<u64>>,
    history: Option<AdmittedWatchReceiver<Option<tokio::time::Instant>>>,
    policy: IngressOwnershipPolicy,
    storage: IngressWorkScope,
}

pub struct AdmittedConversationInputReceiver {
    pending: mpsc::Receiver<AdmittedInput>,
    cancelled: AdmittedWatchReceiver<bool>,
    policy: IngressOwnershipPolicy,
    _storage: IngressWorkScope,
}

struct AdmittedInput {
    input: Option<OwnedWork<ConversationInput>>,
    guard: Option<Arc<dyn ConversationInputGuard>>,
    state: SharedOwned<AtomicU8>,
    delivered: Option<oneshot::Sender<AdmittedConversationSendOutcome>>,
    _storage: IngressWorkScope,
    _input_storage: IngressWorkScope,
}

impl AdmittedInput {
    fn finish(mut self, outcome: AdmittedConversationSendOutcome) {
        let _ = self.delivered.take().expect("one delivery").send(outcome);
    }
}

struct PendingReply {
    receiver: oneshot::Receiver<AdmittedConversationSendOutcome>,
    _storage: IngressWorkScope,
}

impl Future for PendingReply {
    type Output = Result<AdmittedConversationSendOutcome, oneshot::error::RecvError>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::pin::Pin::new(&mut self.receiver).poll(cx)
    }
}

struct CancelOnDrop {
    cancelled: AdmittedWatchSender<bool>,
    state: SharedOwned<AtomicU8>,
    armed: bool,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed && !cancel_pending(&self.state) {
            self.cancelled.send_replace(true);
        }
    }
}

impl std::fmt::Debug for AdmittedConversationInputSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdmittedConversationInputSender")
            .field("closed", &self.inner.is_closed())
            .field("capacity", &self.inner.capacity())
            .finish()
    }
}

impl AdmittedConversationInputSender {
    /// The receiver's driver is returned explicitly; its executor allocation
    /// belongs to the caller's separately admitted task plan.
    pub fn channel(
        capacity: usize,
        policy: IngressOwnershipPolicy,
    ) -> Result<(Self, AdmittedConversationInputReceiver), AdmissionRefusal> {
        if capacity == 0 {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "input queue capacity",
            });
        }
        let storage = IngressWorkScope::new(policy.admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "input queue storage",
                &[
                    sizing::channel_bytes_checked::<AdmittedInput>(capacity, capacity)?,
                    sizing::shared_owner_bytes::<()>(),
                ],
            )?,
        )?);
        let (cancelled, cancellation) =
            AdmittedWatchSender::channel(&policy, IngressClass::Data, false)?;
        let (inner, pending) = mpsc::channel(capacity);
        Ok((
            Self {
                inner,
                cancelled,
                policy: policy.clone(),
                storage: storage.clone(),
                progress: None,
                history: None,
            },
            AdmittedConversationInputReceiver {
                pending,
                cancelled: cancellation,
                policy,
                _storage: storage,
            },
        ))
    }

    /// Watch values remain live. Each send observes the current version, just
    /// like a fresh subscription, without exporting a raw receiver.
    pub fn with_observation(
        mut self,
        observation: DeliveryObservation,
    ) -> Result<Self, AdmissionRefusal> {
        let progress = observation.admitted_progress();
        let history = observation.admitted_history();
        if observation.progress.is_some() != progress.is_some()
            || observation.history.is_some() != history.is_some()
            || progress
                .as_ref()
                .is_some_and(|value| !value.belongs_to(&self.policy))
            || history
                .as_ref()
                .is_some_and(|value| !value.belongs_to(&self.policy))
        {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "input observation ownership",
            });
        }
        self.progress = progress;
        self.history = history;
        Ok(self)
    }

    pub async fn send_classified(
        &self,
        input: OwnedWork<ConversationInput>,
    ) -> AdmittedConversationSendOutcome {
        self.send_inner(input, None).await
    }

    pub async fn send_guarded(
        &self,
        input: OwnedWork<ConversationInput>,
        guard: Arc<dyn ConversationInputGuard>,
    ) -> AdmittedConversationSendOutcome {
        self.send_inner(input, Some(guard)).await
    }

    async fn send_inner(
        &self,
        input: OwnedWork<ConversationInput>,
        guard: Option<Arc<dyn ConversationInputGuard>>,
    ) -> AdmittedConversationSendOutcome {
        use AdmittedConversationSendOutcome::{Ambiguous, PreWriteFailure};
        if !input.belongs_to(&self.policy) {
            return PreWriteFailure(
                AdmissionRefusal::InvalidPlan {
                    phase: "input ownership",
                }
                .into(),
            );
        }
        let refreshing = matches!(&*input, ConversationInput::RefreshContext { .. });
        let observe = refreshing || matches!(&*input, ConversationInput::Configure { .. });
        let deadline = if observe || matches!(&*input, ConversationInput::Steer(_)) {
            Duration::from_secs(15)
        } else {
            DELIVERY_DEADLINE
        };
        let class = if matches!(
            &*input,
            ConversationInput::PermissionAnswer { .. } | ConversationInput::Cancel
        ) {
            IngressClass::Control
        } else {
            IngressClass::Data
        };
        let plan = AllocationPlan::checked(
            "input delivery metadata",
            &[
                // Tokio 1.53.3 oneshot Inner: state, value, two waker cells and Arc.
                size_of::<Option<AdmittedConversationSendOutcome>>() + 12 * size_of::<usize>(),
                sizing::shared_owner_bytes::<AtomicU8>(),
            ],
        )
        .expect("fixed input metadata layout");
        let storage = match self.policy.admit(class, plan) {
            Ok(storage) => IngressWorkScope::new(storage),
            Err(error) => return PreWriteFailure(error.into()),
        };
        let state = storage.initialize(|| AtomicU8::new(PENDING));
        let (delivered, receiver) = oneshot::channel();
        let receipt = PendingReply {
            receiver,
            _storage: storage.clone(),
        };
        let pending = AdmittedInput {
            _input_storage: input.work_scope().expect("validated input ownership"),
            input: Some(input),
            guard,
            state: state.clone(),
            delivered: Some(delivered),
            _storage: storage,
        };
        let started = tokio::time::Instant::now();
        let progress = observe
            .then(|| {
                self.progress
                    .as_ref()
                    .map(AdmittedWatchReceiver::observe_current)
            })
            .flatten();
        let history = observe
            .then(|| {
                self.history
                    .as_ref()
                    .map(AdmittedWatchReceiver::observe_current)
            })
            .flatten();
        let observation = DeliveryObservation::admitted(progress, history);
        if let Err(error) = self.inner.try_send(pending) {
            return PreWriteFailure(AdmittedError::Unavailable(match error {
                mpsc::error::TrySendError::Full(_) => {
                    "the conversation input queue is not being consumed"
                }
                mpsc::error::TrySendError::Closed(_) => "the conversation input task has ended",
            }));
        }
        let mut abort = CancelOnDrop {
            cancelled: self.cancelled.clone(),
            state: state.clone(),
            armed: true,
        };
        let outcome = receive_delivery(
            receipt,
            observation.progress,
            observation.history,
            &state,
            deadline,
            refreshing,
            started,
        )
        .await
        .unwrap_or_else(|detail| {
            let error = AdmittedError::Unavailable(detail);
            if cancel_pending(&state) {
                PreWriteFailure(error)
            } else {
                Ambiguous(error)
            }
        });
        if matches!(outcome, Ambiguous(_)) {
            self.cancelled.send_replace(true);
        }
        abort.armed = false;
        outcome
    }

    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }
    pub fn requested_queue_bytes(&self) -> usize {
        self.storage.requested_bytes()
    }
}

#[derive(Debug)]
pub struct AdmittedConversationChannel {
    input: AdmittedConversationInputSender,
    conversation_id: SharedOwned<String>,
}

impl AdmittedConversationChannel {
    pub fn new(
        input: AdmittedConversationInputSender,
        conversation_id: SharedOwned<String>,
    ) -> Result<Self, AdmissionRefusal> {
        if !conversation_id.belongs_to(&input.policy) {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "conversation identity ownership",
            });
        }
        Ok(Self {
            input,
            conversation_id,
        })
    }

    pub fn input(&self) -> &AdmittedConversationInputSender {
        &self.input
    }
    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }
    pub fn identity(&self) -> &SharedOwned<String> {
        &self.conversation_id
    }
}

#[path = "conversation_admitted_driver.rs"]
mod driver;

#[cfg(test)]
#[path = "conversation_admitted_tests.rs"]
mod tests;
