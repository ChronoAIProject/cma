//! Observe the existing per-provider gate without changing admission or permit lifetime.
use super::{client_types::MAX_SEMAPHORE_WAIT, ProviderError};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Debug)]
pub(crate) struct Gate {
    semaphore: Arc<Semaphore>,
    provider: String,
    capacity: usize,
    state: Mutex<State>,
}
#[derive(Debug, Default)]
struct State {
    next: u64,
    held: usize,
    waiting: BTreeMap<u64, f64>,
}
impl Gate {
    pub(super) fn new(provider: String, capacity: usize) -> Self {
        let gate = Self {
            semaphore: Arc::new(Semaphore::new(capacity)),
            provider,
            capacity,
            state: Mutex::new(State::default()),
        };
        gate.publish(&State::default());
        gate
    }
    fn publish(&self, state: &State) {
        let label = self.provider.clone();
        for (name, value) in [
            ("cmaeg_provider_permit_capacity", self.capacity as f64),
            ("cmaeg_provider_permits_in_use", state.held as f64),
            (
                "cmaeg_provider_permits_available",
                (self.capacity - state.held) as f64,
            ),
            ("cmaeg_provider_permit_waiters", state.waiting.len() as f64),
            (
                "cmaeg_provider_permit_oldest_wait_timestamp_seconds",
                state.waiting.values().copied().next().unwrap_or(0.),
            ),
        ] {
            metrics::gauge!(name, "provider" => label.clone()).set(value);
        }
    }
    pub(super) fn try_acquire(self: &Arc<Self>) -> Result<Permit, ProviderError> {
        let mut wait = Wait::new(self);
        match self.semaphore.clone().try_acquire_owned() {
            Ok(permit) => Ok(wait.acquired(permit)),
            Err(_) => {
                wait.outcome = "full";
                Err(ProviderError::Capacity)
            }
        }
    }
}
pub(super) async fn acquire(gate: &Arc<Gate>) -> Result<Permit, ProviderError> {
    let mut wait = Wait::new(gate);
    match tokio::time::timeout(MAX_SEMAPHORE_WAIT, gate.semaphore.clone().acquire_owned()).await {
        Ok(Ok(permit)) => Ok(wait.acquired(permit)),
        Ok(Err(_)) => {
            wait.outcome = "closed";
            Err(ProviderError::Unavailable(
                "provider semaphore closed".into(),
            ))
        }
        Err(_) => {
            wait.outcome = "timeout";
            Err(ProviderError::Busy {
                retry_after: Some(MAX_SEMAPHORE_WAIT),
            })
        }
    }
}
struct Wait {
    gate: Arc<Gate>,
    id: u64,
    started: Instant,
    outcome: &'static str,
}
impl Wait {
    fn new(gate: &Arc<Gate>) -> Self {
        let mut state = gate.state.lock().unwrap_or_else(|e| e.into_inner());
        let id = state.next;
        state.next = state.next.wrapping_add(1);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        state.waiting.insert(id, timestamp);
        gate.publish(&state);
        Self {
            gate: gate.clone(),
            id,
            started: Instant::now(),
            outcome: "cancelled",
        }
    }
    fn acquired(&mut self, permit: OwnedSemaphorePermit) -> Permit {
        let mut state = self.gate.state.lock().unwrap_or_else(|e| e.into_inner());
        state.waiting.remove(&self.id);
        state.held += 1;
        self.gate.publish(&state);
        self.outcome = "acquired";
        Permit {
            gate: self.gate.clone(),
            permit: Some(permit),
            started: Instant::now(),
        }
    }
}
impl Drop for Wait {
    fn drop(&mut self) {
        let mut state = self.gate.state.lock().unwrap_or_else(|e| e.into_inner());
        state.waiting.remove(&self.id);
        self.gate.publish(&state);
        metrics::counter!("cmaeg_provider_permit_acquisitions_total", "provider" => self.gate.provider.clone(), "outcome" => self.outcome).increment(1);
        metrics::histogram!("cmaeg_provider_permit_wait_seconds", "provider" => self.gate.provider.clone(), "outcome" => self.outcome).record(self.started.elapsed().as_secs_f64());
    }
}
pub(crate) struct Permit {
    gate: Arc<Gate>,
    permit: Option<OwnedSemaphorePermit>,
    started: Instant,
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut state = self.gate.state.lock().unwrap_or_else(|e| e.into_inner());
        // Serialize release with publication so a waking waiter cannot overcount held permits.
        drop(self.permit.take());
        state.held -= 1;
        self.gate.publish(&state);
        metrics::histogram!("cmaeg_provider_permit_held_seconds", "provider" => self.gate.provider.clone()).record(self.started.elapsed().as_secs_f64());
    }
}
#[cfg(test)]
mod tests;
