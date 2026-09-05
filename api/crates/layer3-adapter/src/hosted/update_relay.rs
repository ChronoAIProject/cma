use tokio::sync::mpsc;

use crate::ConversationUpdate;

/// Ordered ingress that never blocks the WebSocket reader on durable folding.
#[derive(Clone)]
pub(super) struct UpdateRelay {
    ingress: mpsc::UnboundedSender<ConversationUpdate>,
}

impl UpdateRelay {
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
    let task = tokio::spawn(async move {
        while let Some(update) = queued.recv().await {
            if output.send(update).await.is_err() {
                break;
            }
        }
    });
    (UpdateRelay { ingress }, task)
}
