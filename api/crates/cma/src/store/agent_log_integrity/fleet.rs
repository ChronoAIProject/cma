//! Fleet selection and bounded scheduling; durable proof remains the authority.
use super::*;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

pub(in crate::store) mod telemetry;
#[cfg(test)]
mod tests;

const PREPARATION_DEADLINE: Duration = Duration::from_secs(10);
const FAILURE_COOLDOWN: Duration = Duration::from_secs(60);
const SUCCESS_COOLDOWN: Duration = Duration::from_secs(30 * 60);
const MAX_CACHED_AGENTS: usize = 1_024;

pub(in crate::store) struct Activation {
    percent: u8,
    slots: Semaphore,
    cooldowns: Mutex<BTreeMap<AgentId, Instant>>,
}

impl Default for Activation {
    fn default() -> Self {
        Self::new(0)
    }
}

#[derive(Clone, Copy)]
enum Prepared {
    Reused,
    Audited,
}

impl Activation {
    fn new(percent: u8) -> Self {
        assert!(
            percent <= 100,
            "fleet percentage must be validated by config"
        );
        Self {
            percent,
            slots: Semaphore::new(1),
            cooldowns: Mutex::new(BTreeMap::new()),
        }
    }

    pub(in crate::store) fn selected(&self, id: &AgentId) -> bool {
        if self.percent == 0 {
            return false;
        }
        let mut digest = Sha256::new();
        digest.update(b"cma.agent-log.fleet.v1\0");
        digest.update(id.as_str().as_bytes());
        let hash = digest.finalize();
        let bucket = u64::from_be_bytes(hash[..8].try_into().expect("SHA-256 prefix")) % 100;
        bucket < u64::from(self.percent)
    }

    async fn prepare<F>(&self, id: &AgentId, operation: F)
    where
        F: std::future::Future<Output = Result<Prepared, ()>>,
    {
        if !self.selected(id) {
            return;
        }
        let now = Instant::now();
        // Cache only scheduling. Neither a positive nor a negative entry grants
        // trust: the caller still validates the fresh durable snapshot.
        if self
            .cooldowns
            .lock()
            .expect("fleet scheduling poisoned")
            .get(id)
            .is_some_and(|until| *until > now)
        {
            return;
        }
        let Ok(_permit) = self.slots.try_acquire() else {
            telemetry::record(telemetry::Event::Busy);
            return;
        };
        if self
            .cooldowns
            .lock()
            .expect("fleet scheduling poisoned")
            .get(id)
            .is_some_and(|until| *until > Instant::now())
        {
            return;
        }
        self.remember(id, now + FAILURE_COOLDOWN);
        let outcome = tokio::time::timeout(PREPARATION_DEADLINE, operation).await;
        match outcome {
            Ok(Ok(prepared)) => {
                self.remember(id, Instant::now() + SUCCESS_COOLDOWN);
                telemetry::record(match prepared {
                    Prepared::Reused => telemetry::Event::Reused,
                    Prepared::Audited => telemetry::Event::Prepared,
                });
            }
            _ => telemetry::record(telemetry::Event::Failed),
        }
        // Cancellation drops the semaphore permit and leaves only this Agent's
        // short cooldown. No background task outlives the requesting future.
    }

    fn remember(&self, id: &AgentId, until: Instant) {
        let mut cache = self.cooldowns.lock().expect("fleet scheduling poisoned");
        if !cache.contains_key(id) && cache.len() >= MAX_CACHED_AGENTS {
            let oldest = cache
                .iter()
                .min_by_key(|(_, expiry)| *expiry)
                .map(|(id, _)| id.clone())
                .expect("nonempty bounded cache");
            cache.remove(&oldest);
        }
        cache.insert(id.clone(), until);
    }
}

impl MongoSandboxStore {
    pub(crate) fn with_fleet_log_percentage(mut self, percent: u8) -> Self {
        self.fleet_log_activation = Arc::new(Activation::new(percent));
        self
    }

    pub(in crate::store) fn bounded_log_enabled(&self, id: &AgentId) -> bool {
        self.bounded_admission_agents.contains(id) || self.fleet_log_activation.selected(id)
    }

    /// Call before opening an append transaction or taking a local writer lock.
    /// Any failure leaves the ordinary append/proof path responsible for the
    /// outcome; quarantine is never bypassed and other Agents remain usable.
    pub(in crate::store) async fn prepare_fleet_log(&self, id: &AgentId) {
        self.fleet_log_activation
            .prepare(id, async {
                let mut session = self
                    .client
                    .start_session()
                    .snapshot(true)
                    .await
                    .map_err(|_| ())?;
                let (agent, trusted) = read::read_proven_agent(self, id, &mut session)
                    .await
                    .map_err(|_| ())?
                    .ok_or(())?;
                if agent.status == layer2_core::sandbox::AgentStatus::Deleted {
                    return Err(());
                }
                if trusted {
                    return Ok(Prepared::Reused);
                }
                drop(session);
                preparation::prepare(self, id).await.map_err(|_| ())?;
                Ok(Prepared::Audited)
            })
            .await;
    }
}
