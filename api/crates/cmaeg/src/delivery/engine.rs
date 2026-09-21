//! Eight bounded workers serialize commands by event without spawning per-send tasks.
use super::{
    event::key, intents::DeliveryCommand, pacing::Pacing, relay_client::RelayClient, send::Sender,
    worker,
};
use crate::{turns::TurnTracker, DeliveryConfig};
use cmaeg_core::seal::MasterKeyRing;
use std::{
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::{
    sync::{mpsc, oneshot},
    task::{JoinHandle, JoinSet},
};
pub const WORKERS: usize = 8;
pub struct DeliveryEngine {
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}
impl Drop for DeliveryEngine {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl DeliveryEngine {
    pub fn start(
        receiver: mpsc::Receiver<DeliveryCommand>,
        keys: Arc<MasterKeyRing>,
        relay: RelayClient,
        config: DeliveryConfig,
        tracker: Arc<TurnTracker>,
        custom: super::custom::CustomDelivery,
    ) -> Self {
        let sender = Arc::new(Sender {
            custom,
            keys,
            relay,
            pacing: Pacing::default(),
            consumed: Mutex::new(Default::default()),
        });
        let (stop, stopping) = oneshot::channel();
        let task = tokio::spawn(run(receiver, stopping, sender, config, tracker));
        Self {
            stop: Some(stop),
            task,
        }
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
    mut receiver: mpsc::Receiver<DeliveryCommand>,
    mut stop: oneshot::Receiver<()>,
    sender: Arc<Sender>,
    config: DeliveryConfig,
    tracker: Arc<TurnTracker>,
) {
    let mut tasks = JoinSet::new();
    let mut workers = Vec::new();
    for _ in 0..WORKERS {
        let (tx, rx) = mpsc::channel(64);
        workers.push(tx);
        tasks.spawn(worker::run(
            rx,
            sender.clone(),
            config.clone(),
            tracker.clone(),
        ));
    }
    let mut closing = false;
    loop {
        tokio::select! {
            biased;
            _ = &mut stop, if !closing => { closing = true; receiver.close(); }
            command = receiver.recv() => {
                let Some(command) = command else { break; };
                let (channel, event) = match &command {
                    DeliveryCommand::Reply { event, .. } | DeliveryCommand::Typing { event, .. } => (&event.channel_id, &event.event_id),
                    DeliveryCommand::Update { message_ref, .. } => (&message_ref.channel_id, &message_ref.event_id),
                    DeliveryCommand::StreamOutcome { turn, .. } => (&turn.channel.id, &turn.activity.event_id),
                };
                let index = key(channel.as_str(), event.as_str())[0] as usize % WORKERS;
                if workers[index].send(command).await.is_err() { break; }
            }
        }
    }
    drop(workers);
    while let Some(result) = tasks.join_next().await {
        if result.is_err() {
            tracing::error!("delivery worker stopped unexpectedly");
        }
    }
}
