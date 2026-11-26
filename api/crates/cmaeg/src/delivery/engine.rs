//! Eight bounded workers serialize commands by event and consumer without per-send tasks.
use super::{
    event::key, intents::DeliveryCommand, pacing::Pacing, queue, relay_client::RelayClient,
    send::Sender, worker,
};
use crate::{
    turns::{
        observer::{ActivityObserver, NoopObserver},
        TurnTracker,
    },
    DeliveryConfig,
};
use cmaeg_core::seal::MasterKeyRing;
use cmaeg_execution::delivery::engine::dispatch;
use std::{
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::{sync::oneshot, task::JoinHandle};
pub const WORKERS: usize = 8;
pub struct DeliveryEngine {
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
    sender: Arc<Sender>,
}
impl Drop for DeliveryEngine {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl DeliveryEngine {
    pub fn start(
        receiver: impl Into<queue::Input<DeliveryCommand>>,
        keys: Arc<MasterKeyRing>,
        relay: RelayClient,
        config: DeliveryConfig,
        tracker: Arc<TurnTracker>,
        custom: super::custom::CustomDelivery,
    ) -> Self {
        Self::start_observed(
            receiver,
            keys,
            relay,
            config,
            tracker,
            custom,
            Arc::new(NoopObserver),
        )
    }

    /// Start the engine reporting every delivery step's outcome to `observer`.
    pub fn start_observed(
        receiver: impl Into<queue::Input<DeliveryCommand>>,
        keys: Arc<MasterKeyRing>,
        relay: RelayClient,
        config: DeliveryConfig,
        tracker: Arc<TurnTracker>,
        custom: super::custom::CustomDelivery,
        observer: Arc<dyn ActivityObserver>,
    ) -> Self {
        let sender = Arc::new(Sender {
            custom,
            keys,
            relay,
            pacing: Pacing::default(),
            consumed: Mutex::new(Default::default()),
            observer,
        });
        let (stop, stopping) = oneshot::channel();
        let task = tokio::spawn(run(
            receiver.into(),
            stopping,
            sender.clone(),
            config,
            tracker,
        ));
        Self {
            stop: Some(stop),
            task,
            sender,
        }
    }
    pub(crate) fn side_messages_if(
        &self,
        enabled: bool,
    ) -> Option<super::side_messages::SideMessageDelivery> {
        enabled.then(|| super::side_messages::SideMessageDelivery::new(&self.sender))
    }

    pub async fn drain(mut self, deadline: Instant) -> bool {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        match tokio::time::timeout_at(deadline.into(), &mut self.task).await {
            Ok(result) => result.is_ok(),
            Err(_) => {
                self.task.abort();
                let _ = (&mut self.task).await;
                false
            }
        }
    }
}
async fn run(
    receiver: queue::Input<DeliveryCommand>,
    stop: oneshot::Receiver<()>,
    sender: Arc<Sender>,
    config: DeliveryConfig,
    tracker: Arc<TurnTracker>,
) {
    dispatch(
        receiver,
        stop,
        WORKERS,
        |command| {
            let (channel, event, consumer) = match command {
                DeliveryCommand::DurableReply(reply) => (
                    &reply.event.channel_id,
                    &reply.event.event_id,
                    reply.event.consumer.as_ref(),
                ),
                DeliveryCommand::Reply { event, .. } | DeliveryCommand::Typing { event, .. } => {
                    (&event.channel_id, &event.event_id, event.consumer.as_ref())
                }
                DeliveryCommand::Update { message_ref, .. } => (
                    &message_ref.channel_id,
                    &message_ref.event_id,
                    message_ref.consumer.as_ref(),
                ),
                DeliveryCommand::StreamOutcome { turn, .. }
                | DeliveryCommand::RetireStream { turn } => {
                    // The worker checks this identity against the authenticated EventRef
                    // before admitting any state or transport effect.
                    (
                        &turn.channel.id,
                        &turn.activity.event_id,
                        turn.consumer.as_ref(),
                    )
                }
            };
            key(channel.as_str(), event.as_str(), consumer)[0] as usize % WORKERS
        },
        move |commands| worker::run(commands, sender.clone(), config.clone(), tracker.clone()),
    )
    .await;
}
