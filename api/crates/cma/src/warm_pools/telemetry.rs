//! Cached, deployment-scoped capacity observations. Scrapes never call the provider.
use super::*;
use std::collections::BTreeMap;

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
            let mut previous = BTreeMap::new();
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
                    observe_capacity_changes(&current, &inventory, &mut previous);
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
        match startup::observed_capacity(target, inventory) {
            Some(ready) => count.ready += u64::from(ready),
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
            metrics::gauge!("cma_warm_pool_capacity_degraded", "kind" => kind)
                .set(f64::from(count.ready < count.desired));
            metrics::gauge!("cma_warm_pool_sample_success_timestamp_seconds", "kind" => kind)
                .set(at);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ObservedCapacity {
    binding: NativePoolBinding,
    revision: i64,
    desired: u32,
    degraded: bool,
}

fn observe_capacity_changes(
    targets: &[&PoolTarget],
    inventory: &[NativePoolStatus],
    previous: &mut BTreeMap<String, ObservedCapacity>,
) {
    previous.retain(|id, _| targets.iter().any(|target| &target.id == id));
    for target in targets {
        if target.target_replicas == 0 {
            previous.remove(&target.id);
            continue;
        }
        let Some(ready) = startup::observed_capacity(target, inventory) else {
            continue;
        };
        let Some(binding) = target.binding.clone() else {
            continue;
        };
        let current = ObservedCapacity {
            binding,
            revision: target.revision,
            desired: target.target_replicas,
            degraded: ready < target.target_replicas,
        };
        let old = previous.insert(target.id.clone(), current.clone());
        let kind = if target.machine { "machine" } else { "agent" };
        if current.degraded && old.as_ref() != Some(&current) {
            tracing::warn!(target: "cma_warm_pool_capacity", parent: None,
                kind, desired = current.desired, ready,
                reason = "provider_reports_insufficient_ready", "Native warm pool capacity degraded");
        } else if !current.degraded
            && old.is_some_and(|old| {
                old.degraded
                    && old.binding == current.binding
                    && old.revision == current.revision
                    && old.desired == current.desired
            })
        {
            tracing::info!(target: "cma_warm_pool_capacity", parent: None,
                kind, desired = current.desired, ready, "Native warm pool capacity recovered");
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

pub(crate) fn startup(outcome: &'static str) {
    metrics::counter!("cma_warm_pool_startup_total", "outcome" => outcome).increment(1);
}

#[cfg(test)]
mod tests;
