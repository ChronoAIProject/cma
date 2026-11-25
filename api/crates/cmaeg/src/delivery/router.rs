//! The injectable entry point of delivery.
//!
//! Turn runners, stream consumers and tools hand commands to a
//! [`DeliverySink`] instead of a concrete queue, so a routed scheduler can
//! replace today's engine. The trait documents the routed contract;
//! [`ShardedScheduler`], today's engine, is the only implementation.
use super::intents::{DeliveryCommand, IntentReceipt};
use crate::{ingress::launch::LaunchRequest, turns::outcomes::TurnOutcome};
use futures_util::future::BoxFuture;
use std::sync::Arc;
use tokio::sync::mpsc::{self, error::TrySendError};

pub trait DeliverySink: Send + Sync {
    /// Provider streams: emits this outcome's Sync frame, then waits only on this consumer's mailbox
    /// and on its record's, owner's and the gateway's byte budgets.
    fn submit(&self, c: DeliveryCommand) -> BoxFuture<'_, Result<(), Refused>>;
    /// Tool, ingress and notice commands: never waits; emits a Sync tool-reply frame after reserving its queue slot.
    fn try_submit(&self, c: DeliveryCommand) -> Result<(), Refusal>;
    /// A turn's terminal failure (timeout, stream loss): never waits; never refused on the routed lane.
    fn terminal(&self, turn: Arc<LaunchRequest>, outcome: TurnOutcome);
    /// Synchronous part-cap check (§R3.1); the legacy implementation returns None.
    fn preflight(&self, c: &DeliveryCommand) -> Option<IntentReceipt>;
}

/// Why a command was not queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    Capacity,
    Closed,
}

/// A refused `try_submit`. The command is handed back so the caller can complete any receipt
/// outside every lock (Amendment R3 §R3.1).
#[derive(Debug)]
pub struct Refusal {
    pub reason: Refused,
    pub command: Box<DeliveryCommand>,
}

/// Today's engine: the 256-command inbound queue in front of the 8 sharded workers.
pub struct ShardedScheduler {
    inbound: Inbound,
    _monitor: super::queue::Monitor,
}
enum Inbound {
    Raw(mpsc::Sender<DeliveryCommand>),
    Measured(super::queue::Sender<DeliveryCommand>),
}
impl Inbound {
    async fn send(
        &self,
        command: DeliveryCommand,
    ) -> Result<(), mpsc::error::SendError<DeliveryCommand>> {
        match self {
            Self::Raw(sender) => sender.send(command).await,
            Self::Measured(sender) => sender.send(command).await,
        }
    }
    fn try_send(&self, command: DeliveryCommand) -> Result<(), Refusal> {
        let result = match self {
            Self::Raw(sender) => sender.try_send(command),
            Self::Measured(sender) => sender.try_send(command),
        };
        result.map_err(|error| {
            let (reason, command) = match error {
                TrySendError::Full(command) => (Refused::Capacity, command),
                TrySendError::Closed(command) => (Refused::Closed, command),
            };
            Refusal {
                reason,
                command: Box::new(command),
            }
        })
    }
    fn measured(&self) -> bool {
        matches!(self, Self::Measured(_))
    }
}
impl ShardedScheduler {
    /// Create the production channel with exact queued depth, residence and oldest age.
    pub fn channel(capacity: usize) -> (Self, super::queue::Input<DeliveryCommand>) {
        let inventory = Arc::new(super::queue::Inventory::new("inbound"));
        let monitor = inventory.monitor();
        let (sender, receiver) = super::queue::channel(capacity, inventory);
        metrics::gauge!("cmaeg_delivery_queue_capacity", "stage" => "inbound").set(capacity as f64);
        (
            Self {
                inbound: Inbound::Measured(sender),
                _monitor: monitor,
            },
            receiver.into(),
        )
    }

    pub fn new(inbound: mpsc::Sender<DeliveryCommand>) -> Self {
        let monitor = super::queue::inbound_monitor(&inbound);
        Self {
            inbound: Inbound::Raw(inbound),
            _monitor: monitor,
        }
    }
}
impl DeliverySink for ShardedScheduler {
    /// Waits for room in the inbound queue. Stream commands carry no receipt
    /// a caller must complete, so a closed queue is reported without them.
    fn submit(&self, c: DeliveryCommand) -> BoxFuture<'_, Result<(), Refused>> {
        Box::pin(async move {
            if self.inbound.measured() {
                return self.inbound.send(c).await.map_err(|_| Refused::Closed);
            }
            let mut wait = super::queue::Wait::new("inbound");
            let result = self.inbound.send(c).await.map_err(|_| Refused::Closed);
            wait.finish(if result.is_ok() { "accepted" } else { "closed" });
            result
        })
    }
    /// Never drops a refused command itself, so a receipt it carries is never
    /// completed implicitly, possibly under a caller's lock.
    fn try_submit(&self, c: DeliveryCommand) -> Result<(), Refusal> {
        let mut wait = (!self.inbound.measured()).then(|| super::queue::Wait::new("inbound"));
        let result = self.inbound.try_send(c);
        if let Some(wait) = &mut wait {
            wait.finish(match &result {
                Ok(_) => "accepted",
                Err(refusal) => match refusal.reason {
                    Refused::Capacity => "full",
                    Refused::Closed => "closed",
                },
            });
        }
        result
    }
    fn terminal(&self, turn: Arc<LaunchRequest>, outcome: TurnOutcome) {
        // A stalled delivery sink must not keep a timed-out turn alive indefinitely.
        let command = DeliveryCommand::StreamOutcome {
            turn,
            outcome,
            answer_response_id: None,
            accepted_deadline: None,
            recovery: None,
        };
        if let Err(refusal) = self.try_submit(command) {
            let command = *refusal.command;
            tracing::warn!(
                reason = failure_reason(&command),
                "turn failure could not reach delivery sink"
            );
        }
    }
    fn preflight(&self, _c: &DeliveryCommand) -> Option<IntentReceipt> {
        None
    }
}

/// The failure code a terminal outcome carries, or its kind.
fn failure_reason(command: &DeliveryCommand) -> &str {
    match command {
        DeliveryCommand::StreamOutcome {
            outcome: TurnOutcome::Failed { code, .. },
            ..
        } => code,
        DeliveryCommand::StreamOutcome { outcome, .. } => outcome.kind(),
        _ => "not_a_turn_outcome",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delivery::intents::{DeliveryOutcome, ReplySource};
    use cmaeg_core::{
        channel::ChannelDefinition,
        ids::ChannelId,
        message::{OutboundMessage, RenderingOutcome, RenderingReport},
        seal::{ChannelPointer, EventRefPayload, ReplyAddress},
    };
    use tokio::sync::oneshot;

    fn reply(
        text: &str,
        trace: tracing::Span,
    ) -> (DeliveryCommand, oneshot::Receiver<IntentReceipt>) {
        let definition: ChannelDefinition = serde_json::from_str(include_str!(
            "../../../cmaeg-core/tests/fixtures/channel.json"
        ))
        .unwrap();
        let channel = crate::channels::resolver::ResolvedChannel {
            id: ChannelId::new("cmaeg1.ch.router-fixture").unwrap(),
            pointer: ChannelPointer {
                v: 1,
                record_id: definition.record_id.clone(),
                record_url: "https://cma.example/api/v1/gateway/channel-records/record-1".into(),
            },
            definition,
            agent_key_hash: [0; 32],
            consumer_keys: Vec::new(),
            loaded_at: std::time::Instant::now(),
            version: 1,
        };
        let event = EventRefPayload {
            telegram_admission: None,
            v: 1,
            activity_sha256: None,
            channel_id: channel.id.clone(),
            record_id: channel.pointer.record_id.clone(),
            event_id: "msg-1".parse().unwrap(),
            partition_key: "msg-1".into(),
            conversation_id: "conversation-1".parse().unwrap(),
            expires_at: i64::MAX,
            reply: ReplyAddress::None,
            consumer: None,
            lineage: None,
        };
        let (receipt, received) = oneshot::channel();
        let command = DeliveryCommand::Reply {
            trace,
            channel: Arc::new(channel),
            event,
            message: OutboundMessage {
                whatsapp_template: None,
                structured_content: None,
                reply_with_received_sticker: false,
                clear_native_attachments: false,
                native_media_update: None,
                native_attachments: Vec::new(),
                thread_reply: false,
                text: Some(text.into()),
                blocks: vec![],
                reply_to: None,
            },
            source: ReplySource::Tool,
            receipt,
        };
        (command, received)
    }

    /// The handed-back command is the refused one, with its trace and
    /// payload, and its receipt sender is still usable.
    fn assert_handed_back(refusal: Refusal, reason: Refused, span: &tracing::Span, text: &str) {
        assert_eq!(refusal.reason, reason);
        let DeliveryCommand::Reply {
            trace,
            message,
            receipt,
            ..
        } = *refusal.command
        else {
            panic!("a different command was handed back");
        };
        assert!(span.id().is_some());
        assert_eq!(trace.id(), span.id());
        assert_eq!(message.text.as_deref(), Some(text));
        receipt
            .send(IntentReceipt {
                message_parts: vec![],
                message_ref: None,
                rendering: RenderingReport {
                    outcome: RenderingOutcome::Unsupported,
                    blocks: vec![],
                },
                delivery: DeliveryOutcome::Rejected {
                    reason: "delivery_capacity".into(),
                },
            })
            .expect("the receipt sender was dropped with the refusal");
    }

    /// A subscriber that gives every span an id, so a trace can be compared.
    fn traced() -> tracing::subscriber::DefaultGuard {
        tracing::subscriber::set_default(tracing_subscriber::fmt().finish())
    }

    #[tokio::test]
    async fn a_full_queue_hands_the_command_back_with_its_receipt() {
        let _traced = traced();
        let (inbound, mut queued) = mpsc::channel(1);
        let scheduler = ShardedScheduler::new(inbound);
        let (first, _first_receipt) = reply("first", tracing::Span::none());
        assert!(scheduler.try_submit(first).is_ok());
        let span = tracing::info_span!("refused");
        let (second, received) = reply("second", span.clone());
        assert_handed_back(
            scheduler.try_submit(second).unwrap_err(),
            Refused::Capacity,
            &span,
            "second",
        );
        assert!(received.await.is_ok());
        assert!(queued.recv().await.is_some());
        let (third, _) = reply("third", tracing::Span::none());
        assert!(scheduler.preflight(&third).is_none());
    }

    #[tokio::test]
    async fn a_closed_queue_hands_the_command_back_and_refuses_submissions() {
        let _traced = traced();
        let (inbound, queued) = mpsc::channel(4);
        drop(queued);
        let scheduler = ShardedScheduler::new(inbound);
        let span = tracing::info_span!("refused");
        let (command, received) = reply("late", span.clone());
        assert_handed_back(
            scheduler.try_submit(command).unwrap_err(),
            Refused::Closed,
            &span,
            "late",
        );
        assert!(received.await.is_ok());
        let (streamed, _) = reply("streamed", tracing::Span::none());
        assert_eq!(scheduler.submit(streamed).await, Err(Refused::Closed));
    }

    #[tokio::test]
    async fn submit_waits_for_room_in_the_queue() {
        let (inbound, mut queued) = mpsc::channel(1);
        let scheduler = Arc::new(ShardedScheduler::new(inbound));
        let (first, _) = reply("first", tracing::Span::none());
        scheduler.submit(first).await.unwrap();
        let (second, _second_receipt) = reply("second", tracing::Span::none());
        let waiting = {
            let scheduler = scheduler.clone();
            tokio::spawn(async move { scheduler.submit(second).await })
        };
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        assert!(queued.recv().await.is_some());
        assert_eq!(waiting.await.unwrap(), Ok(()));
    }
    #[tokio::test]
    async fn production_scheduler_records_one_submission_per_attempt() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let _local = metrics::set_default_local_recorder(&recorder);
        let (scheduler, mut queued) = ShardedScheduler::channel(1);
        let (first, _) = reply("first", tracing::Span::none());
        scheduler.submit(first).await.unwrap();
        let (second, _) = reply("second", tracing::Span::none());
        assert_eq!(
            scheduler.try_submit(second).unwrap_err().reason,
            Refused::Capacity
        );
        assert!(queued.recv().await.is_some());
        let (third, _) = reply("third", tracing::Span::none());
        scheduler.try_submit(third).unwrap();
        drop(queued);
        let text = handle.render();
        for (outcome, count) in [("accepted", 2), ("full", 1)] {
            assert!(
                text.lines().any(|line| line
                    .starts_with("cmaeg_delivery_queue_submissions_total{")
                    && line.contains("stage=\"inbound\"")
                    && line.contains(&format!("outcome=\"{outcome}\""))
                    && line.ends_with(&format!(" {count}"))),
                "{text}"
            );
        }
        assert!(text.contains("cmaeg_delivery_queue_depth{stage=\"inbound\"} 0"));
    }
}
