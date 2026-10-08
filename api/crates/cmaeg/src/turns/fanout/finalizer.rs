//! A tracked wait for existing delivery capacity; ingress never waits here.
use crate::{
    delivery::{
        intents::DeliveryCommand,
        router::{DeliverySink, Refused},
    },
    telemetry::names,
};
use std::{sync::Arc, time::Instant};
use tokio::sync::oneshot;

pub(super) async fn run(
    rx: oneshot::Receiver<Vec<DeliveryCommand>>,
    delivery: Arc<dyn DeliverySink>,
    deadline: Instant,
    channel_hash: String,
) {
    let commands = match rx.await {
        Ok(commands) => commands,
        Err(_) => {
            metrics::counter!(names::FANOUT_FAILED_DROPPED, "reason" => "unanswered").increment(1);
            tracing::warn!(
                channel_hash,
                reason = "unanswered",
                "fan-out acceptance ended without a failure list"
            );
            return;
        }
    };
    let mut commands = commands.into_iter();
    while let Some(command) = commands.next() {
        let DeliveryCommand::ConsumerFailed { consumer, code, .. } = &command else {
            unreachable!("the finalizer only owns consumer failures");
        };
        let consumer = consumer.clone();
        let code = *code;
        // timeout_at can poll a ready send after expiry; do not enqueue late.
        let result = if Instant::now() >= deadline {
            None
        } else {
            tokio::time::timeout_at(deadline.into(), delivery.submit(command))
                .await
                .ok()
        };
        let reason = match result {
            Some(Ok(())) => continue,
            Some(Err(Refused::Closed)) => "closed",
            Some(Err(Refused::Capacity)) => {
                tracing::warn!(channel_hash, %consumer, code, "unexpected capacity refusal from awaiting delivery submit");
                "closed"
            }
            None => "deadline",
        };
        dropped(reason, &channel_hash, &consumer, code);
        if reason == "deadline" {
            undelivered(&commands.collect::<Vec<_>>(), reason, &channel_hash);
            return;
        }
    }
}

pub(super) fn undelivered(commands: &[DeliveryCommand], reason: &'static str, channel_hash: &str) {
    for command in commands {
        if let DeliveryCommand::ConsumerFailed { consumer, code, .. } = command {
            dropped(reason, channel_hash, consumer, code);
        }
    }
}
fn dropped(
    reason: &'static str,
    channel_hash: &str,
    consumer: &cmaeg_core::ids::ConsumerId,
    code: &'static str,
) {
    metrics::counter!(names::FANOUT_FAILED_DROPPED, "reason" => reason).increment(1);
    tracing::warn!(channel_hash, %consumer, code, reason, "fan-out consumer failure could not reach delivery");
}
