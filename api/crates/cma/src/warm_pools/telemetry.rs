//! Cached, deployment-scoped capacity observations. Scrapes never call the provider.
use super::*;

impl NativeWarmPools {
    pub(crate) fn spawn_observations(
        self: Arc<Self>,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        for kind in ["agent", "machine"] {
            metrics::gauge!("cma_warm_pool_observation_complete", "kind" => kind).set(0.);
            metrics::gauge!("cma_warm_pool_sample_success_timestamp_seconds", "kind" => kind)
                .set(0.);
        }
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(15));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = crate::shutdown::signalled(shutdown.clone()) => return,
                    _ = tick.tick() => {}
                }
                let sample = async {
                    let targets = self.store.list().await?;
                    let provider = self
                        .adapter
                        .native_pools()
                        .ok_or(WarmPoolError::Unavailable)?;
                    let inventory = provider
                        .inventory()
                        .await
                        .map_err(|_| WarmPoolError::Unavailable)?;
                    let mut current = Vec::new();
                    for target in &targets {
                        if !target.retiring && !target.retired && self.current_plan(target)? {
                            current.push(target);
                        }
                    }
                    let counts = capacity(&current, &inventory);
                    Ok::<_, WarmPoolError>((targets, counts))
                };
                let result = tokio::select! {
                    _ = crate::shutdown::signalled(shutdown.clone()) => return,
                    result = tokio::time::timeout(Duration::from_secs(8), sample) => result
                };
                let at = crate::telemetry::timestamp();
                metrics::gauge!("cma_warm_pool_sample_timestamp_seconds").set(at);
                match result {
                    Ok(Ok((targets, counts))) => {
                        let now = mongodb::bson::DateTime::now();
                        let due: Vec<_> = targets
                            .iter()
                            .filter(|target| {
                                !target.retired
                                    && target.lease_until <= now
                                    && target.reconcile_after <= now
                            })
                            .collect();
                        metrics::gauge!("cma_worker_durable_items", "worker" => "native_warm_pools", "state" => "due").set(due.len() as f64);
                        let oldest = due
                            .iter()
                            .map(|target| target.reconcile_after.timestamp_millis())
                            .min()
                            .unwrap_or(0);
                        metrics::gauge!("cma_worker_durable_oldest_timestamp_seconds", "worker" => "native_warm_pools", "state" => "due").set(oldest as f64 / 1000.);
                        crate::store::worker_observations::sampled(
                            "native_warm_pools",
                            "due",
                            true,
                        );
                        for (kind, count) in ["agent", "machine"].into_iter().zip(counts) {
                            publish(kind, Some(count), at);
                        }
                    }
                    _ => {
                        crate::store::worker_observations::sampled(
                            "native_warm_pools",
                            "due",
                            false,
                        );
                        for kind in ["agent", "machine"] {
                            publish(kind, None, at);
                        }
                    }
                }
            }
        })
    }
}

#[derive(Clone, Copy, Default)]
struct Capacity {
    desired: u64,
    ready: u64,
    complete: bool,
}

fn capacity(targets: &[&PoolTarget], inventory: &[NativePoolStatus]) -> [Capacity; 2] {
    let mut counts = [Capacity {
        complete: true,
        ..Default::default()
    }; 2];
    for target in targets {
        let count = &mut counts[usize::from(target.machine)];
        count.desired += u64::from(target.target_replicas);
        // A disabled, never-created target deliberately has no provider resource.
        if target.target_replicas == 0 && target.binding.is_none() {
            continue;
        }
        let exact = inventory
            .iter()
            .find(|status| target.binding.as_ref() == Some(&status.binding));
        match exact {
            Some(status)
                if status.observed && status.desired_replicas == target.target_replicas =>
            {
                count.ready += u64::from(status.ready_replicas);
            }
            _ => count.complete = false,
        }
    }
    counts
}

fn publish(kind: &'static str, capacity: Option<Capacity>, at: f64) {
    let complete = capacity.is_some_and(|count| count.complete);
    metrics::gauge!("cma_warm_pool_observation_complete", "kind" => kind).set(f64::from(complete));
    if let Some(count) = capacity {
        metrics::gauge!("cma_warm_pool_desired_replicas", "kind" => kind).set(count.desired as f64);
        if complete {
            metrics::gauge!("cma_warm_pool_ready_replicas", "kind" => kind).set(count.ready as f64);
            metrics::gauge!("cma_warm_pool_sample_success_timestamp_seconds", "kind" => kind)
                .set(at);
        }
    }
}

pub(super) fn applied(target: &PoolTarget, success: bool) {
    let kind = if target.machine { "machine" } else { "agent" };
    metrics::counter!("cma_warm_pool_apply_total", "kind" => kind, "outcome" => if success { "ok" } else { "error" }).increment(1);
    // A periodic re-apply of an already acknowledged revision is not new progress.
    if success && target.applied_revision != target.revision {
        metrics::gauge!("cma_warm_pool_revision_applied_timestamp_seconds", "kind" => kind)
            .set(crate::telemetry::timestamp());
    }
}

#[cfg(test)]
mod tests;
