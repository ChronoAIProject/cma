//! Queue residence starts only after reserving capacity. Dropped receivers and
//! cancelled tasks release their entries without retaining a command or identity.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::mpsc;

#[derive(Default)]
pub(super) struct Inventory(Mutex<Entries>);
#[derive(Default)]
struct Entries {
    next: u64,
    queued: BTreeMap<u64, Instant>,
}
impl Inventory {
    fn sample(&self) {
        let state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        publish(&state);
    }
    fn enter(self: &Arc<Self>) -> Residence {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        let id = state.next;
        state.next = state.next.wrapping_add(1);
        let started = Instant::now();
        state.queued.insert(id, started);
        publish(&state);
        Residence {
            inventory: self.clone(),
            id,
            started,
        }
    }
    pub(super) fn monitor(self: &Arc<Self>) -> Monitor {
        let weak = Arc::downgrade(self);
        Monitor(tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                let Some(inventory) = weak.upgrade() else {
                    break;
                };
                inventory.sample();
            }
        }))
    }
}
fn publish(state: &Entries) {
    metrics::gauge!("cmaeg_delivery_queue_sample_timestamp_seconds", "stage" => "worker")
        .set(timestamp());
    metrics::gauge!("cmaeg_delivery_queue_depth", "stage" => "worker")
        .set(state.queued.len() as f64);
    let age = state
        .queued
        .values()
        .min()
        .map_or(0., |at| at.elapsed().as_secs_f64());
    metrics::gauge!("cmaeg_delivery_queue_oldest_seconds", "stage" => "worker").set(age);
}
struct Residence {
    inventory: Arc<Inventory>,
    id: u64,
    started: Instant,
}
impl Drop for Residence {
    fn drop(&mut self) {
        let mut state = self
            .inventory
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.queued.remove(&self.id);
        publish(&state);
    }
}
struct Queued<C> {
    command: C,
    residence: Residence,
}
pub(super) struct Sender<C> {
    sender: mpsc::Sender<Queued<C>>,
    inventory: Arc<Inventory>,
}
pub(super) struct Receiver<C>(mpsc::Receiver<Queued<C>>);
pub(super) fn channel<C>(capacity: usize, inventory: Arc<Inventory>) -> (Sender<C>, Receiver<C>) {
    let (sender, receiver) = mpsc::channel(capacity);
    (Sender { sender, inventory }, Receiver(receiver))
}
impl<C> Sender<C> {
    pub(super) async fn send(&self, command: C) -> Result<(), mpsc::error::SendError<C>> {
        let mut wait = Wait::new("worker");
        match self.sender.reserve().await {
            Ok(permit) => {
                permit.send(Queued {
                    command,
                    residence: self.inventory.enter(),
                });
                wait.finish("accepted");
                Ok(())
            }
            Err(_) => {
                wait.finish("closed");
                Err(mpsc::error::SendError(command))
            }
        }
    }
}
impl<C> Receiver<C> {
    pub(super) async fn recv(&mut self) -> Option<C> {
        let queued = self.0.recv().await?;
        metrics::histogram!("cmaeg_delivery_queue_residence_seconds", "stage" => "worker")
            .record(queued.residence.started.elapsed().as_secs_f64());
        Some(queued.command)
    }
    pub(super) fn is_closed(&self) -> bool {
        self.0.is_closed()
    }
}

pub(super) struct Monitor(tokio::task::JoinHandle<()>);
impl Drop for Monitor {
    fn drop(&mut self) {
        self.0.abort();
    }
}
pub(super) fn inbound_monitor<C: Send + 'static>(sender: &mpsc::Sender<C>) -> Monitor {
    let weak = sender.downgrade();
    Monitor(tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let Some(sender) = weak.upgrade() else {
                metrics::gauge!("cmaeg_delivery_queue_sample_timestamp_seconds", "stage" => "inbound").set(timestamp());
                metrics::gauge!("cmaeg_delivery_queue_depth", "stage" => "inbound").set(0.);
                break;
            };
            metrics::gauge!("cmaeg_delivery_queue_sample_timestamp_seconds", "stage" => "inbound")
                .set(timestamp());
            metrics::gauge!("cmaeg_delivery_queue_depth", "stage" => "inbound")
                .set((sender.max_capacity() - sender.capacity()) as f64);
            metrics::gauge!("cmaeg_delivery_queue_capacity", "stage" => "inbound")
                .set(sender.max_capacity() as f64);
        }
    }))
}

pub(super) struct Wait {
    stage: &'static str,
    started: Instant,
    outcome: &'static str,
}
impl Wait {
    pub(super) fn new(stage: &'static str) -> Self {
        metrics::gauge!("cmaeg_delivery_queue_waiters", "stage" => stage).increment(1.);
        Self {
            stage,
            started: Instant::now(),
            outcome: "cancelled",
        }
    }
    pub(super) fn finish(&mut self, outcome: &'static str) {
        self.outcome = outcome;
    }
}
impl Drop for Wait {
    fn drop(&mut self) {
        metrics::gauge!("cmaeg_delivery_queue_waiters", "stage" => self.stage).decrement(1.);
        metrics::counter!("cmaeg_delivery_queue_submissions_total", "stage" => self.stage, "outcome" => self.outcome).increment(1);
        metrics::histogram!("cmaeg_delivery_queue_wait_seconds", "stage" => self.stage, "outcome" => self.outcome).record(self.started.elapsed().as_secs_f64());
    }
}

#[cfg(test)]
mod tests;

fn timestamp() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
