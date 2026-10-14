//! Retire existing unowned Profile warmups; native pools own new prewarming.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::sandbox::{SandboxStoreError, WarmSandboxState};
use layer2_core::{now_rfc3339, rfc3339_at};
use layer3_adapter::SandboxAdapter;
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::MissedTickBehavior;

use crate::store::{MongoSandboxStore, WarmSandboxClaim};

mod cleanup;
pub(super) use cleanup::retire_environment;
#[cfg(test)]
mod hierarchy;
#[cfg(test)]
mod preparation;
#[cfg(test)]
pub(super) use hierarchy::prepare_step as prepare_hierarchy_step;
#[cfg(test)]
pub(super) use preparation::prepare_environment;

pub(crate) struct LegacyWarmSandboxCleanup {
    pub store: Arc<MongoSandboxStore>,
    pub adapter: Arc<dyn SandboxAdapter>,
}

impl LegacyWarmSandboxCleanup {
    pub(crate) fn spawn(self, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let worker = Arc::new(self);
            let mut jobs = JoinSet::new();
            let mut ticker = tokio::time::interval(Duration::from_secs(2));
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                if *shutdown.borrow() {
                    break;
                }
                tokio::select! {
                    _ = shutdown.changed() => break,
                    result = jobs.join_next(), if !jobs.is_empty() => {
                        if let Some(Err(error)) = result { tracing::error!(%error, "warmup worker task failed"); }
                    }
                    _ = ticker.tick() => (),
                    _ = worker.store.warmup_changed.notified() => (),
                }
                if *shutdown.borrow() {
                    break;
                }
                if let Err(error) = worker.store.retire_legacy_warmups(&now_rfc3339()).await {
                    tracing::error!(%error, "legacy warmup retirement scheduling failed");
                    continue;
                }
                while jobs.len() < 10 {
                    let claim = worker
                        .store
                        .claim_legacy_warm_cleanup(&now_rfc3339(), &after(60))
                        .await;
                    match claim {
                        Ok(Some(claim)) => {
                            let worker = worker.clone();
                            jobs.spawn(async move {
                                if let Err(error) = worker.run_claim(claim).await {
                                    tracing::warn!(%error, "warmup step will be reconciled");
                                }
                            });
                        }
                        Ok(None) => break,
                        Err(error) => {
                            tracing::error!(%error, "warmup claim failed");
                            break;
                        }
                    }
                }
            }
            // Cleanup authority survives shutdown. No new runtime is allocated;
            // the next process resumes the same durable retirement debt.
            jobs.abort_all();
            while jobs.join_next().await.is_some() {}
        })
    }

    async fn run_claim(&self, claim: WarmSandboxClaim) -> Result<(), SandboxStoreError> {
        let operation = retire_environment(self.adapter.as_ref(), &claim.record);
        tokio::pin!(operation);
        let mut heartbeat = tokio::time::interval(Duration::from_secs(20));
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let result = loop {
            tokio::select! {
                result = &mut operation => break result,
                _ = heartbeat.tick() => self.store.renew_warmup_work(&claim, &after(60)).await?,
            }
        };
        let (next, delay) = match result {
            Ok(next) => (next, 0),
            Err(_) => {
                // Provider/install error text may include command output. Only
                // the resource identity and neutral failure are logged here.
                tracing::warn!(sandbox_id = %claim.record.id, "legacy warm environment cleanup failed; retaining cleanup debt");
                let mut next = claim.record.clone();
                next.state = WarmSandboxState::Retiring;
                (next, 30)
            }
        };
        self.store
            .finish_warmup_work(&claim, next, &now_rfc3339(), &after(delay))
            .await
    }
}

fn after(seconds: u64) -> String {
    rfc3339_at(SystemTime::now() + Duration::from_secs(seconds))
}
