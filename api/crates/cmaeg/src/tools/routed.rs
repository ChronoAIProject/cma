//! Routed sends retain today's queue and workers after their caller's deadline.
use super::{reply_targets::receipts, ToolsState};
use crate::{
    delivery::intents::{DeliveryCommand, IntentReceipt},
    http::request_start::RequestStart,
};
use cmaeg_core::{
    ids::ConsumerId,
    problem::{Problem, ProblemCode},
};
use std::time::Duration;
use tokio::{
    sync::oneshot,
    time::{timeout_at, Instant},
};
use tracing::Instrument;

/// Routed sends count from request receipt; legacy sends from `deliver` entry.
pub(super) const TOOL_DEADLINE: Duration = Duration::from_secs(8);

impl ToolsState {
    pub(super) async fn deliver_routed(
        &self,
        start: RequestStart,
        command: DeliveryCommand,
        received: oneshot::Receiver<IntentReceipt>,
    ) -> Result<IntentReceipt, Problem> {
        let deadline = start.0 + TOOL_DEADLINE;
        let (channel, event_id) = command.event_key();
        let consumer = command.consumer().map_or("default", ConsumerId::as_str);
        let span = tracing::info_span!("cmaeg.deliver", channel = %crate::telemetry::channel_hash(channel.as_str()), event_id = %event_id, consumer);
        async {
            if Instant::now() >= deadline {
                return Err(timeout());
            }
            // The existing mpsc send is cancel-safe: timing out here admits nothing.
            timeout_at(deadline, self.delivery.submit(command))
                .await
                .map_err(|_| timeout())?
                .map_err(|_| unavailable())?;
            match timeout_at(deadline, received).await {
                Ok(Ok(receipt)) => Ok(receipt),
                Ok(Err(_)) => Err(unavailable()),
                // Queue admission transferred responsibility to the worker. The
                // receipt's receiver may go away, but the send must still run.
                Err(_) => Ok(receipts::uncertain("delivery_in_progress")),
            }
        }
        .instrument(span)
        .await
    }
}

fn timeout() -> Problem {
    Problem::new(ProblemCode::Internal, 504, "delivery_timeout")
}
fn unavailable() -> Problem {
    Problem::new(ProblemCode::Internal, 503, "delivery_unavailable")
}
