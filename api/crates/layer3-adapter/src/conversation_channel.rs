use std::future::Future;
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use crate::{AdapterError, ConversationInput};

const DELIVERY_DEADLINE: Duration = Duration::from_secs(2);

struct PendingInput {
    input: ConversationInput,
    guard: Option<Arc<dyn ConversationInputGuard>>,
    state: Arc<AtomicU8>,
    delivered: oneshot::Sender<ConversationSendOutcome>,
}

const PENDING: u8 = 0;
const WRITING: u8 = 1;
const CANCELLED: u8 = 2;

/// Recheck held authority after queueing, immediately before the ordered writer
/// invokes its runtime handler. Implementations must not renew expired evidence
/// or perform runtime effects. A refusal leaves this conversation usable.
#[async_trait::async_trait]
pub trait ConversationInputGuard: Send + Sync {
    async fn check(&self) -> crate::Result<()>;
}

/// Read-only live capability evidence. Retained runtime images can refuse a new
/// settings operation without interrupting their current conversation.
#[async_trait::async_trait]
pub trait ConversationSettingsSupport: Send + Sync {
    async fn system_prompt_supported(&self) -> bool;
    async fn managed_context_receipt(&self, _sequence: u64) -> Option<String> {
        None
    }
    async fn managed_transport_receipt(&self, sequence: u64) -> Option<String> {
        self.managed_context_receipt(sequence).await
    }
}

/// What is known about one attempt to hand input to the runtime link.
#[derive(Debug)]
pub enum ConversationSendOutcome {
    PreWriteFailure(AdapterError),
    WriteConfirmed,
    Ambiguous(AdapterError),
}

struct AbortOnDrop {
    handle: tokio::task::AbortHandle,
    state: Arc<AtomicU8>,
    armed: bool,
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if self.armed && !cancel_pending(&self.state) {
            self.handle.abort();
        }
    }
}

/// Sending half of a conversation channel.
#[derive(Clone)]
pub struct ConversationInputSender {
    inner: mpsc::Sender<PendingInput>,
    abort: tokio::task::AbortHandle,
    settings_support: Option<Arc<dyn ConversationSettingsSupport>>,
}

impl std::fmt::Debug for ConversationInputSender {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConversationInputSender")
            .field("closed", &self.inner.is_closed())
            .field("capacity", &self.inner.capacity())
            .finish()
    }
}

impl ConversationInputSender {
    /// Spawn the single ordered writer for this conversation.
    pub fn spawn<F, Fut>(capacity: usize, mut handler: F) -> Self
    where
        F: FnMut(ConversationInput) -> Fut + Send + 'static,
        Fut: Future<Output = crate::Result<()>> + Send + 'static,
    {
        let (inner, mut pending) = mpsc::channel::<PendingInput>(capacity);
        let task = tokio::spawn(async move {
            while let Some(PendingInput {
                input,
                guard,
                state,
                mut delivered,
            }) = pending.recv().await
            {
                if delivered.is_closed() || state.load(Ordering::Acquire) == CANCELLED {
                    continue;
                }
                if let Some(guard) = guard {
                    let checked = tokio::select! {
                        biased;
                        _ = delivered.closed() => continue,
                        checked = guard.check() => checked,
                    };
                    if let Err(error) = checked {
                        let _ = delivered.send(ConversationSendOutcome::PreWriteFailure(error));
                        continue;
                    }
                }
                // Cancellation wins only while no handler can have run. This
                // CAS makes a timeout/refusal safe even after queue admission.
                if state
                    .compare_exchange(PENDING, WRITING, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
                {
                    continue;
                }
                let configuring = matches!(
                    input,
                    ConversationInput::Configure { .. }
                        | ConversationInput::RefreshContext { .. }
                        | ConversationInput::Steer(_)
                );
                let refreshing = matches!(input, ConversationInput::RefreshContext { .. });
                let result = handler(input).await;
                // These handlers reserve Unsupported for a definitive refusal
                // before any runtime mutation; later failures are ambiguous.
                let refused = configuring && matches!(result, Err(AdapterError::Unsupported(_)))
                    || refreshing
                        && matches!(result, Err(AdapterError::OperationIdentityConflict(_)));
                let failed = result.is_err() && !refused;
                let outcome = match result {
                    Err(error) if refused => ConversationSendOutcome::PreWriteFailure(error),
                    Ok(()) => ConversationSendOutcome::WriteConfirmed,
                    Err(error) => ConversationSendOutcome::Ambiguous(error),
                };
                let _ = delivered.send(outcome);
                if failed {
                    pending.close();
                    break;
                }
            }
        });
        Self {
            inner,
            abort: task.abort_handle(),
            settings_support: None,
        }
    }

    pub fn with_settings_support(mut self, support: Arc<dyn ConversationSettingsSupport>) -> Self {
        self.settings_support = Some(support);
        self
    }

    pub async fn system_prompt_supported(&self) -> bool {
        match &self.settings_support {
            Some(support) => support.system_prompt_supported().await,
            None => false,
        }
    }

    pub async fn managed_context_receipt(&self, sequence: u64) -> Option<String> {
        self.settings_support
            .as_ref()?
            .managed_context_receipt(sequence)
            .await
    }

    pub async fn managed_transport_receipt(&self, sequence: u64) -> Option<String> {
        self.settings_support
            .as_ref()?
            .managed_transport_receipt(sequence)
            .await
    }

    /// Wait for the local writer to confirm the operation was written.
    pub async fn send(&self, input: ConversationInput) -> crate::Result<()> {
        match self.send_classified(input).await {
            ConversationSendOutcome::WriteConfirmed => Ok(()),
            ConversationSendOutcome::PreWriteFailure(error)
            | ConversationSendOutcome::Ambiguous(error) => Err(error),
        }
    }

    /// Distinguish refusal before handler invocation from uncertain delivery.
    pub async fn send_classified(&self, input: ConversationInput) -> ConversationSendOutcome {
        let deadline = if matches!(
            input,
            ConversationInput::Configure { .. }
                | ConversationInput::RefreshContext { .. }
                | ConversationInput::Steer(_)
        ) {
            Duration::from_secs(15)
        } else {
            DELIVERY_DEADLINE
        };
        self.send_with_deadline(input, None, deadline).await
    }

    pub async fn send_guarded(
        &self,
        input: ConversationInput,
        guard: Arc<dyn ConversationInputGuard>,
    ) -> ConversationSendOutcome {
        let deadline = if matches!(
            input,
            ConversationInput::Configure { .. }
                | ConversationInput::RefreshContext { .. }
                | ConversationInput::Steer(_)
        ) {
            Duration::from_secs(15)
        } else {
            DELIVERY_DEADLINE
        };
        self.send_with_deadline(input, Some(guard), deadline).await
    }

    async fn send_with_deadline(
        &self,
        input: ConversationInput,
        guard: Option<Arc<dyn ConversationInputGuard>>,
        deadline: Duration,
    ) -> ConversationSendOutcome {
        let (delivered, receipt) = oneshot::channel();
        let state = Arc::new(AtomicU8::new(PENDING));
        if let Err(error) = self.inner.try_send(PendingInput {
            input,
            guard,
            state: state.clone(),
            delivered,
        }) {
            let error = match error {
                mpsc::error::TrySendError::Full(_) => AdapterError::Unavailable(
                    "the conversation input queue is not being consumed".into(),
                ),
                mpsc::error::TrySendError::Closed(_) => {
                    AdapterError::Unavailable("the conversation input task has ended".into())
                }
            };
            return ConversationSendOutcome::PreWriteFailure(error);
        }

        // Once handler invocation starts, cancellation is ambiguous and retires
        // the generation. A still-pending request can be cancelled independently.
        let mut abort = AbortOnDrop {
            handle: self.abort.clone(),
            state: state.clone(),
            armed: true,
        };
        let outcome = match tokio::time::timeout(deadline, receipt).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => {
                incomplete_delivery(&state, "the conversation input task ended before delivery")
            }
            Err(_) => incomplete_delivery(
                &state,
                "the conversation input task did not deliver the operation",
            ),
        };
        if matches!(outcome, ConversationSendOutcome::Ambiguous(_)) {
            self.abort.abort();
        }
        abort.armed = false;
        outcome
    }
}

fn cancel_pending(state: &AtomicU8) -> bool {
    state
        .compare_exchange(PENDING, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
        || state.load(Ordering::Acquire) == CANCELLED
}

fn incomplete_delivery(state: &AtomicU8, detail: &str) -> ConversationSendOutcome {
    let error = AdapterError::Unavailable(detail.into());
    if cancel_pending(state) {
        ConversationSendOutcome::PreWriteFailure(error)
    } else {
        ConversationSendOutcome::Ambiguous(error)
    }
}

/// Bidirectional attachment to one ACP conversation.
#[derive(Debug)]
pub struct ConversationChannel {
    pub input: ConversationInputSender,
    pub conversation_id: Option<String>,
}

#[cfg(test)]
#[path = "conversation_guard_tests.rs"]
mod guard_tests;
