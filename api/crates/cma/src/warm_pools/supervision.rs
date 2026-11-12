//! Keep optional capacity reconciliation separate from scheduler-loop health.
use super::NativeWarmPools;
use std::{sync::Arc, time::Duration};

impl NativeWarmPools {
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
