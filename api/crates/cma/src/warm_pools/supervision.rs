//! Keep optional capacity reconciliation separate from scheduler-loop health.
use super::{reconcile_claimed, NativeWarmPools, WarmPoolError};
use std::{sync::Arc, time::Duration};

impl NativeWarmPools {
    pub(super) async fn reconcile_one(&self) -> Result<bool, WarmPoolError> {
        let Some(provider) = self.adapter.native_pools() else {
            return Ok(false);
        };
        let target = if self.default_holds.lane(false) || self.default_holds.lane(true) {
            let mut claimed = None;
            for target in self.store.list().await? {
                if !self.default_holds.lane(target.machine) {
                    claimed = self.store.claim_retirement(&target.id).await?;
                    if claimed.is_some() {
                        break;
                    }
                }
            }
            claimed
        } else {
            self.store.claim().await?
        };
        let Some(target) = target else {
            return Ok(false);
        };
        if target.retiring {
            return self.retire_claimed(provider, target).await;
        }
        if !self.current_plan(&target)? {
            if !self.store.candidate_protected(&target).await? {
                return self.retire_claimed(provider, target).await;
            }
            return self.reconcile_candidate(provider, target).await;
        }
        reconcile_claimed(self.store.as_ref(), provider, target, self.target_fencing)
            .await
            .map(|_| true)
    }

    pub(crate) fn spawn_supervised(
        self: Arc<Self>,
        shutdown: tokio::sync::watch::Receiver<bool>,
        activity: crate::worker_supervision::Activity,
    ) -> tokio::task::JoinHandle<()> {
        let owned = (self, tokio::task::JoinSet::<()>::new());
        crate::worker_supervision::spawn(
            owned,
            activity,
            shutdown,
            |(pools, tasks), activity, mut shutdown| {
                Box::pin(async move {
                    let telemetry = crate::telemetry::worker::Worker::start("native_warm_pools");
                    let mut tick = tokio::time::interval(Duration::from_secs(5));
                    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    loop {
                        activity.waiting(
                            Duration::from_secs(5),
                            if tasks.is_empty() {
                                "idle_scan"
                            } else {
                                "reconciliation"
                            },
                        );
                        telemetry.heartbeat();
                        if *shutdown.borrow() {
                            break;
                        }
                        tokio::select! {
                            result = shutdown.changed() => {
                                if result.is_err() || *shutdown.borrow() { break; }
                            }
                            result = tasks.join_next(), if !tasks.is_empty() => {
                                if let Some(Err(error)) = result {
                                    crate::telemetry::worker::completed("native_warm_pools", "reconciliation", "terminated");
                                    tracing::error!(%error, "warm pool reconciliation child terminated");
                                }
                            }
                            _ = tick.tick() => {
                                if tasks.is_empty() {
                                    let pools = pools.clone();
                                    let activity = activity.clone();
                                    tasks.spawn(async move {
                                        let _phase = activity.reconciliation();
                                        for _ in 0..20 {
                                            let result = pools.reconcile_one().await;
                                            crate::telemetry::worker::completed("native_warm_pools", "execution", match &result { Ok(true) => "attempted", Ok(false) => "idle", Err(_) => "error" });
                                            match result {
                                                Ok(true) => {},
                                                Ok(false) => break,
                                                Err(_) => { tracing::warn!("native warm pool reconciliation will retry"); break; }
                                            }
                                        }
                                    });
                                }
                            }
                        }
                        activity.scheduled();
                    }
                })
            },
            |(_, tasks)| {
                Box::pin(async move {
                    // Writes have no blanket operation deadline. Shutdown remains
                    // bounded by the existing coordinator's total drain deadline.
                    while tasks.join_next().await.is_some() {}
                })
            },
        )
    }
}
