use tokio::sync::mpsc;

use crate::ConversationUpdate;

/// Ordered ingress that never blocks the WebSocket reader on durable folding.
#[derive(Clone)]
pub(super) struct UpdateRelay {
    ingress: mpsc::UnboundedSender<ConversationUpdate>,
}

impl UpdateRelay {
    /// Collect the domain effect of one frame without publishing before its receipt.
    pub(super) fn capture() -> (Self, mpsc::UnboundedReceiver<ConversationUpdate>) {
        let (ingress, receiver) = mpsc::unbounded_channel();
        (Self { ingress }, receiver)
    }

    pub(super) fn send(&self, update: ConversationUpdate) {
        if self.ingress.send(update).is_err() {
            tracing::debug!("conversation update relay has ended");
        }
    }
}

pub(super) fn spawn(
    output: mpsc::Sender<ConversationUpdate>,
) -> (UpdateRelay, tokio::task::JoinHandle<()>) {
    let (ingress, mut queued) = mpsc::unbounded_channel();
    let authority = crate::owner_protocol::current_authority();
    let task = tokio::spawn(async move {
        let relay = async {
            while let Some(update) = queued.recv().await {
                if output.send(update).await.is_err() {
                    break;
                }
            }
        };
        match authority {
            Some(authority) => tokio::select! { biased;
                _ = authority.revoked() => (),
                _ = relay => (),
            },
            None => relay.await,
        }
    });
    (UpdateRelay { ingress }, task)
}
