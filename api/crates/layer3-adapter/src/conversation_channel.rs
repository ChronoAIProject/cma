use std::future::Future;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use crate::{AdapterError, ConversationInput};

const DELIVERY_DEADLINE: Duration = Duration::from_secs(2);

struct PendingInput {
    input: ConversationInput,
    delivered: oneshot::Sender<crate::Result<()>>,
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
    armed: bool,
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.handle.abort();
        }
    }
}

/// Sending half of a conversation channel.
#[derive(Clone)]
pub struct ConversationInputSender {
    inner: mpsc::Sender<PendingInput>,
    abort: tokio::task::AbortHandle,
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
            while let Some(PendingInput { input, delivered }) = pending.recv().await {
                let result = handler(input).await;
                let failed = result.is_err();
                let _ = delivered.send(result);
                if failed {
                    pending.close();
                    break;
                }
            }
        });
        Self {
            inner,
            abort: task.abort_handle(),
        }
    }

    /// Wait for the local writer to confirm the operation was written.
    pub async fn send(&self, input: ConversationInput) -> crate::Result<()> {
        match self.send_classified(input).await {
            ConversationSendOutcome::WriteConfirmed => Ok(()),
            ConversationSendOutcome::PreWriteFailure(error)
            | ConversationSendOutcome::Ambiguous(error) => Err(error),
        }
    }

    /// Preserve the only retry-safe boundary: rejection before queue admission.
    pub async fn send_classified(&self, input: ConversationInput) -> ConversationSendOutcome {
        self.send_with_deadline(input, DELIVERY_DEADLINE).await
    }

    async fn send_with_deadline(
        &self,
        input: ConversationInput,
        deadline: Duration,
    ) -> ConversationSendOutcome {
        let (delivered, receipt) = oneshot::channel();
        if let Err(error) = self.inner.try_send(PendingInput { input, delivered }) {
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

        // Cancellation after queue admission makes delivery ambiguous. Retire
        // this writer generation so a caller cannot continue through it.
        let mut abort = AbortOnDrop {
            handle: self.abort.clone(),
            armed: true,
        };
        let outcome = match tokio::time::timeout(deadline, receipt).await {
            Ok(Ok(Ok(()))) => ConversationSendOutcome::WriteConfirmed,
            Ok(Ok(Err(error))) => ConversationSendOutcome::Ambiguous(error),
            Ok(Err(_)) => ConversationSendOutcome::Ambiguous(AdapterError::Unavailable(
                "the conversation input task ended before delivery".into(),
            )),
            Err(_) => ConversationSendOutcome::Ambiguous(AdapterError::Unavailable(
                "the conversation input task did not deliver the operation".into(),
            )),
        };
        if !matches!(outcome, ConversationSendOutcome::WriteConfirmed) {
            self.abort.abort();
        }
        abort.armed = false;
        outcome
    }
}

/// Bidirectional attachment to one ACP conversation.
#[derive(Debug)]
pub struct ConversationChannel {
    pub input: ConversationInputSender,
    pub conversation_id: Option<String>,
}
