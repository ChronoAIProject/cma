//! Decoupling the hosted socket from durable conversation folding.
//!
//! A runtime can produce updates much faster than the durable log can commit
//! them. The WebSocket reader must nevertheless keep polling the socket: that
//! is where ping/pong is serviced and where the prompt's terminal JSON-RPC
//! response arrives. This relay preserves the wire order while moving the
//! potentially slow bounded-channel wait onto its own task.

use tokio::sync::mpsc;

use crate::ConversationUpdate;

/// An ordered, non-blocking ingress for updates read from one hosted socket.
#[derive(Clone)]
pub(super) struct UpdateRelay {
    ingress: mpsc::UnboundedSender<ConversationUpdate>,
}

impl UpdateRelay {
    /// Queue one update without making the socket reader wait for persistence.
    pub(super) fn send(&self, update: ConversationUpdate) {
        if self.ingress.send(update).is_err() {
            tracing::debug!("conversation update relay has already ended");
        }
    }
}

/// Forward one socket generation's updates into the core-owned bounded lane.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn gap(frames: u64) -> ConversationUpdate {
        ConversationUpdate::Lost { frames }
    }

    #[tokio::test]
    async fn a_full_core_lane_does_not_block_socket_ingress_and_order_is_preserved() {
        let (output, mut received) = mpsc::channel(1);
        let (relay, task) = spawn(output);

        for frames in 1..=256 {
            relay.send(gap(frames));
        }
        relay.send(ConversationUpdate::TurnEnded(crate::PromptResponse::new(
            crate::TurnStopReason::EndTurn,
        )));
        drop(relay);

        for expected in 1..=256 {
            let update = received.recv().await.expect("the relay stays ordered");
            assert!(
                matches!(update, ConversationUpdate::Lost { frames } if frames == expected),
                "update {expected} arrived out of order: {update:?}"
            );
        }
        assert!(matches!(
            received.recv().await,
            Some(ConversationUpdate::TurnEnded(_))
        ));
        assert!(received.recv().await.is_none());
        task.await.expect("the relay exits cleanly");
    }
}
