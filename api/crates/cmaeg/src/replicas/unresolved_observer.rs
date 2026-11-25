//! Periodic store-only uncertainty observation beside the existing frontier scan.
use crate::{
    ingress::routing_store::RecoveryPage,
    replicas::diagnostics::{LogLimit, ScanError, Stage},
    turns::routed::scheduler::RecoverySource,
};
use futures_util::future::BoxFuture;
use std::sync::Arc;
use tokio::time::Duration;
trait Observation: Send + Sync {
    fn observe(&self) -> BoxFuture<'_, Result<(), super::Error>>;
}
impl Observation for super::unresolved::Operator {
    fn observe(&self) -> BoxFuture<'_, Result<(), super::Error>> {
        Box::pin(self.observe_heads())
    }
}
pub(crate) struct Observer {
    source: Arc<dyn RecoverySource>,
    task: tokio::task::JoinHandle<()>,
}
impl Observer {
    pub fn new(source: Arc<dyn RecoverySource>, operator: super::unresolved::Operator) -> Self {
        Self::with_observation(source, Arc::new(operator))
    }
    fn with_observation(
        source: Arc<dyn RecoverySource>,
        observation: Arc<dyn Observation>,
    ) -> Self {
        let task = tokio::spawn(async move {
            let mut logs = LogLimit::default();
            loop {
                let result = tokio::time::timeout(Duration::from_secs(5), observation.observe())
                    .await
                    .unwrap_or(Err(super::Error::Unavailable));
                if let Err(error) = result {
                    metrics::counter!("cmaeg_replica_uncertainty_observation_failures_total")
                        .increment(1);
                    logs.prune();
                    ScanError::new(error, Stage::Observation, None).report_scan(&mut logs);
                }
                // No catch-up bursts. At most five seconds of observation per minute.
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        });
        Self { source, task }
    }
}
impl Drop for Observer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl RecoverySource for Observer {
    fn start_measurement(&self) -> Option<crate::telemetry::provider_start::Measurement> {
        self.source.start_measurement()
    }

    fn page<'a>(
        &'a self,
        after: Option<&'a str>,
    ) -> BoxFuture<'a, Result<RecoveryPage, ScanError>> {
        self.source.page(after)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::Instant;
    struct Empty;
    impl RecoverySource for Empty {
        fn page<'a>(
            &'a self,
            _: Option<&'a str>,
        ) -> BoxFuture<'a, Result<RecoveryPage, ScanError>> {
            Box::pin(async {
                Ok(RecoveryPage {
                    plans: vec![],
                    issues: vec![],
                    next_cursor: None,
                })
            })
        }
    }
    struct Blocked(tokio::sync::Notify);
    impl Observation for Blocked {
        fn observe(&self) -> BoxFuture<'_, Result<(), crate::replicas::Error>> {
            self.0.notify_one();
            Box::pin(std::future::pending())
        }
    }
    #[tokio::test(start_paused = true)]
    async fn page_latency_excludes_blocked_observation() {
        let observation = Arc::new(Blocked(tokio::sync::Notify::new()));
        let observer = Observer::with_observation(Arc::new(Empty), observation.clone());
        let start = Instant::now();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), observer.page(None))
                .await
                .is_ok(),
            "discovery waited for observation IO"
        );
        assert!(start.elapsed() < Duration::from_millis(100));
        observation.0.notified().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(100), observer.page(None))
                .await
                .is_ok()
        );
    }
}
