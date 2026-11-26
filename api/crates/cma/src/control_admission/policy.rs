use std::{collections::BTreeMap, sync::Mutex, time::Duration};

use super::budget::identity;
use super::*;

#[async_trait::async_trait]
pub(crate) trait SharedControlRate: Send + Sync {
    async fn charge(&self, key: &str, per_minute: u32, burst: u32) -> Result<Option<u64>, ()>;
}

#[async_trait::async_trait]
impl SharedControlRate for crate::store::request_limits::MongoRequestLimits {
    async fn charge(&self, key: &str, per_minute: u32, burst: u32) -> Result<Option<u64>, ()> {
        self.charge_with_burst(key, per_minute, burst, 1)
            .await
            .map_err(|_| ())
    }
}

#[async_trait::async_trait]
pub(crate) trait LifecycleAdmissionPolicy: Send + Sync {
    fn enabled(&self) -> bool;
    async fn admit(
        &self,
        caller: &str,
        target: &str,
        class: Class,
    ) -> Result<Option<StagePermit>, Refusal>;
}

pub(crate) struct Policy {
    config: ControlAdmissionConfig,
    resources: Arc<ResourceBudget>,
    store: Arc<dyn SharedControlRate>,
    clock: Arc<dyn AdmissionClock>,
    cooldowns: Mutex<BTreeMap<[u8; 32], [Instant; 2]>>,
}

impl Policy {
    #[cfg(test)]
    pub(super) fn resources(&self) -> &Arc<ResourceBudget> {
        &self.resources
    }
    pub(crate) fn new(
        config: ControlAdmissionConfig,
        store: Arc<dyn SharedControlRate>,
        clock: Arc<dyn AdmissionClock>,
    ) -> Self {
        Self {
            config,
            resources: ResourceBudget::new(config),
            store,
            clock,
            cooldowns: Mutex::new(BTreeMap::new()),
        }
    }

    fn locally_eligible(
        &self,
        caller: [u8; 32],
        class: Class,
        now: Instant,
    ) -> Result<(), Refusal> {
        let mut cooldowns = self.cooldowns.lock().unwrap();
        // Evict only fully refilled entries: memory pressure cannot mint a fresh
        // allowance. Authoritative cross-replica charging still happens below.
        cooldowns.retain(|_, deadlines| deadlines.iter().any(|deadline| *deadline > now));
        if let Some(deadlines) = cooldowns.get(&caller) {
            if deadlines[class.index()] > now {
                return Err(Refusal::Caller(
                    deadlines[class.index()]
                        .duration_since(now)
                        .as_secs()
                        .saturating_add(1),
                ));
            }
        } else if cooldowns.len() >= self.config.max_callers {
            return Err(Refusal::Capacity);
        }
        // Reserve map space before awaiting; no unbounded pending insertion.
        let deadlines = cooldowns.entry(caller).or_insert([now; 2]);
        deadlines[class.index()] = now + Duration::from_secs(2);
        Ok(())
    }
}

#[async_trait::async_trait]
impl LifecycleAdmissionPolicy for Policy {
    fn enabled(&self) -> bool {
        self.config.enabled
    }

    async fn admit(
        &self,
        caller: &str,
        target: &str,
        class: Class,
    ) -> Result<Option<StagePermit>, Refusal> {
        if !self.config.enabled {
            return Ok(None);
        }
        let permit = self.resources.try_stage(caller, target, class, 1)?;
        let now = self.clock.now();
        let caller = identity(caller);
        self.locally_eligible(caller, class, now)?;
        let rate = match class {
            Class::Stop => self.config.stop_per_minute,
            Class::Delete => self.config.delete_per_minute,
        };
        let hash: String = caller.iter().map(|byte| format!("{byte:02x}")).collect();
        let key = format!("agent-control-v1:{}:{hash}", class.label());
        let store = self.store.clone();
        let burst = self.config.burst;
        let context = layer2_core::diagnostic_context::DiagnosticContext::current();
        let mut charging = tokio::spawn(context.scope(async move {
            let result = store.charge(&key, rate, burst).await;
            (result, permit)
        }));
        let result = tokio::time::timeout(Duration::from_secs(2), &mut charging).await;
        let (delay, outcome) = match result {
            Ok(Ok((Ok(None), permit))) => (
                Duration::from_millis(u64::from(60_000_u32.div_ceil(rate))),
                Ok(Some(permit)),
            ),
            Ok(Ok((Ok(Some(delay)), _permit))) => (
                Duration::from_secs(delay.clamp(1, 60)),
                Err(Refusal::Caller(delay.clamp(1, 60))),
            ),
            _ => {
                tracing::warn!(
                    class = class.label(),
                    "Lifecycle request budget unavailable"
                );
                (Duration::from_secs(1), Err(Refusal::Unavailable))
            }
        };
        if let Some(deadlines) = self.cooldowns.lock().unwrap().get_mut(&caller) {
            deadlines[class.index()] = self.clock.now() + delay;
        }
        outcome
    }
}

#[cfg(test)]
mod tests;
