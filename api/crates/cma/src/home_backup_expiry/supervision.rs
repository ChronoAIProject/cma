use super::*;

impl ExpiryWorker {
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
            |(worker, tasks), activity, mut shutdown| {
                Box::pin(async move {
                    let telemetry = crate::telemetry::worker::Worker::start("home_backup_expiry");
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
                                if let Some(Err(_)) = result {
                                    tracing::warn!("home backup expiry child terminated; durable authority is retained");
                                }
                            }
                            _ = tick.tick() => {
                                if tasks.is_empty() {
                                    let worker = worker.clone();
                                    let activity = activity.clone();
                                    tasks.spawn(async move {
                                        let _phase = activity.reconciliation();
                                        if worker.reconcile_orphan_page().await.is_err() {
                                            crate::telemetry::worker::completed("home_backup_expiry", "orphan_inventory", "error");
                                            tracing::warn!("home backup orphan discovery remains incomplete; durable deletions continue");
                                        }
                                        for _ in 0..20 {
                                            let result = worker.reconcile_one().await;
                                            crate::telemetry::worker::completed("home_backup_expiry", "execution", match &result { Ok(true) => "attempted", Ok(false) => "idle", Err(_) => "error" });
                                            match result {
                                                Ok(true) => {},
                                                Ok(false) => break,
                                                Err(_) => {
                                                    tracing::warn!("home backup expiry will retry with durable authority");
                                                    break;
                                                }
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
            |(_, tasks)| Box::pin(async move { while tasks.join_next().await.is_some() {} }),
        )
    }
}
