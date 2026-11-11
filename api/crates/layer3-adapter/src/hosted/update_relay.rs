use tokio::sync::mpsc;

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
}

impl UpdateRelay {
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
        matches!(self.ingress, Ingress::Bounded(_))
    }

    pub(super) async fn closed(&self) {
        match &self.ingress {
            Ingress::Legacy(ingress) => ingress.closed().await,
            Ingress::Bounded(ingress) => ingress.closed().await,
        }
    }

    pub(super) async fn send(&self, update: ConversationUpdate) {
        let sent = match &self.ingress {
            Ingress::Legacy(ingress) => ingress.send(update),
            Ingress::Bounded(ingress) => ingress.send(update).await,
        };
        if sent.is_err() {
            tracing::debug!("conversation update relay has ended");
        }
    }
}

#[cfg(test)]
#[path = "update_relay_tests.rs"]
mod tests;

pub(super) fn spawn(
    output: mpsc::Sender<ConversationUpdate>,
    bounded: bool,
) -> (UpdateRelay, tokio::task::JoinHandle<()>) {
    if bounded {
        let (ingress, mut queued) = mpsc::channel(RELAY_CAPACITY);
        let task = tokio::spawn(async move {
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
    let task = tokio::spawn(async move {
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
