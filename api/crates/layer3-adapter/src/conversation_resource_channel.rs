use super::*;
use crate::resource_execution_protocol::{RuntimeExecutionControl, StartObservationV1};

#[path = "conversation_resource_input.rs"]
mod input;
pub use input::{ResourceDispatch, ResourceInput};

/// A receiver observation is distinct from a successful transport write.
#[derive(Debug)]
pub enum ResourceSendOutcome {
    /// Definitive refusal before native entry; a lost transport reply is uncertain.
    Refused(AdapterError),
    Observed(Box<StartObservationV1>),
    Uncertain(AdapterError),
}

/// Bound to one runtime, Agent and owner. The ticket-only R1 admission method
/// remains unsupported unless the implementation has an immutable input binding.
#[async_trait::async_trait]
pub trait ResourceConversationTransport: RuntimeExecutionControl {
    async fn send_resource(&self, input: &ResourceDispatch) -> ResourceSendOutcome;
}

pub(super) struct ResourcePending {
    dispatch: ResourceDispatch,
    observed: oneshot::Sender<StartObservationV1>,
}

impl ResourcePending {
    pub(super) async fn deliver(
        self,
        transport: Option<&Arc<dyn ResourceConversationTransport>>,
    ) -> ConversationSendOutcome {
        let Some(transport) = transport else {
            return ConversationSendOutcome::PreWriteFailure(AdapterError::Unsupported(
                "resource execution transport is unavailable".into(),
            ));
        };
        match transport.send_resource(&self.dispatch).await {
            ResourceSendOutcome::Refused(error) => ConversationSendOutcome::PreWriteFailure(error),
            ResourceSendOutcome::Uncertain(error) => ConversationSendOutcome::Ambiguous(error),
            ResourceSendOutcome::Observed(observation) => {
                if observation.validate_for(self.dispatch.ticket()).is_err() {
                    return ConversationSendOutcome::Ambiguous(AdapterError::Unavailable(
                        "resource execution returned mismatched start evidence".into(),
                    ));
                }
                let _ = self.observed.send(*observation);
                ConversationSendOutcome::WriteConfirmed
            }
        }
    }
}

impl ConversationInputSender {
    /// Install one resource handler on this same ordered writer. Legacy inputs
    /// retain their original handler and all resource inputs retain the guard.
    pub fn with_resource_transport(
        self,
        transport: Arc<dyn ResourceConversationTransport>,
    ) -> crate::Result<Self> {
        if !transport.capabilities().is_some_and(|capability| {
            capability.protocol == crate::resource_execution_protocol::PROTOCOL_VERSION
                && capability.targeted_native_stop
                && capability.mediated_effects
        }) {
            return Err(AdapterError::Unsupported(
                "resource execution capability is unavailable".into(),
            ));
        }
        self.resource_transport.set(transport).map_err(|_| {
            AdapterError::Rejected("resource execution transport is already bound".into())
        })?;
        Ok(self)
    }

    pub async fn send_resource_guarded(
        &self,
        dispatch: ResourceDispatch,
        guard: Arc<dyn ConversationInputGuard>,
    ) -> ResourceSendOutcome {
        let started = tokio::time::Instant::now();
        let (observed, observation) = oneshot::channel();
        let (delivered, receipt) = oneshot::channel();
        let state = Arc::new(AtomicU8::new(PENDING));
        if self
            .inner
            .try_send(PendingInput {
                input: PendingPayload::Resource(ResourcePending { dispatch, observed }),
                guard: Some(guard),
                state: state.clone(),
                delivered,
            })
            .is_err()
        {
            return ResourceSendOutcome::Refused(AdapterError::Unavailable(
                "the conversation input queue is unavailable".into(),
            ));
        }
        let mut abort = AbortOnDrop {
            handle: self.abort.clone(),
            state: state.clone(),
            armed: true,
        };
        let delivered = receive_delivery(
            receipt,
            None,
            None,
            &state,
            DELIVERY_DEADLINE,
            false,
            started,
        )
        .await
        .unwrap_or_else(|detail| incomplete_delivery(&state, detail));
        if matches!(delivered, ConversationSendOutcome::Ambiguous(_)) {
            self.abort.abort();
        }
        abort.armed = false;
        match delivered {
            ConversationSendOutcome::PreWriteFailure(error) => ResourceSendOutcome::Refused(error),
            ConversationSendOutcome::Ambiguous(error) => ResourceSendOutcome::Uncertain(error),
            ConversationSendOutcome::WriteConfirmed => match observation.await {
                Ok(observation) => ResourceSendOutcome::Observed(Box::new(observation)),
                Err(_) => ResourceSendOutcome::Uncertain(AdapterError::Unavailable(
                    "resource execution start evidence was lost".into(),
                )),
            },
        }
    }
}

#[cfg(test)]
#[path = "conversation_resource_channel_tests.rs"]
mod tests;
