use super::*;
use crate::store::allocation_buckets::Lane;

impl MetricsWorkers {
    pub(super) async fn maintain_allocation_history(&self) {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            for lane in [Lane::Live, Lane::Backfill] {
                let started = std::time::Instant::now();
                let result = tokio::time::timeout(
                    Duration::from_secs(40),
                    self.store.maintain_allocation_buckets(
                        self.store.view_policy.allocation_rollups_enabled,
                        lane,
                    ),
                )
                .await;
                // This synchronous root keeps JSON formatters from inheriting
                // ambient request/tenant context; never hold it across await.
                let root = if matches!(&result, Ok(Ok(_))) {
                    tracing::info_span!(parent:None, "usage_allocation_maintenance")
                } else {
                    tracing::warn_span!(parent:None, "usage_allocation_maintenance")
                };
                if root.is_disabled() {
                    continue;
                }
                let _entered = root.enter();
                match result {
                    Ok(Ok(done)) if done.claimed => tracing::info!(parent:&root,
                        lane=lane.label(), states=done.states, source_rows=done.source_rows,
                        reused=done.reused, written=done.written, pass_complete=done.pass_complete,
                        elapsed_ms=started.elapsed().as_millis() as u64,
                        outcome="published", "Usage allocation maintenance completed"),
                    Ok(Ok(_)) => {}
                    Ok(Err(_)) => tracing::warn!(parent:&root, lane=lane.label(),
                        elapsed_ms=started.elapsed().as_millis() as u64,
                        outcome="unavailable", "Usage allocation maintenance incomplete"),
                    Err(_) => tracing::warn!(parent:&root, lane=lane.label(),
                        elapsed_ms=started.elapsed().as_millis() as u64,
                        outcome="deadline", "Usage allocation maintenance incomplete"),
                }
            }
        }
    }
}
