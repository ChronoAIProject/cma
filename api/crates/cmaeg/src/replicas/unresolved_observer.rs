//! Periodic store-only uncertainty observation beside the existing frontier scan.
use crate::{
    ingress::routing_store::{AdmissionError, RecoveryPage},
    turns::routed::scheduler::RecoverySource,
};
use futures_util::future::BoxFuture;
use std::sync::Arc;
use tokio::{
    sync::Mutex,
    time::{Duration, Instant},
};
pub(crate) struct Observer {
    pub source: Arc<dyn RecoverySource>,
    pub operator: super::unresolved::Operator,
    last: Mutex<Instant>,
}
impl Observer {
    pub fn new(source: Arc<dyn RecoverySource>, operator: super::unresolved::Operator) -> Self {
        Self {
            source,
            operator,
            last: Mutex::new(Instant::now() - Duration::from_secs(10)),
        }
    }
}
impl RecoverySource for Observer {
    fn page<'a>(
        &'a self,
        after: Option<&'a str>,
    ) -> BoxFuture<'a, Result<RecoveryPage, AdmissionError>> {
        Box::pin(async move {
            let due = {
                let mut last = self.last.lock().await;
                if last.elapsed() >= Duration::from_secs(10) {
                    *last = Instant::now();
                    true
                } else {
                    false
                }
            };
            if due
                && !matches!(
                    tokio::time::timeout(Duration::from_secs(5), self.operator.observe_heads())
                        .await,
                    Ok(Ok(_))
                )
            {
                metrics::counter!("cmaeg_replica_uncertainty_observation_failures_total")
                    .increment(1);
                tracing::warn!(
                    "replica uncertainty inspection incomplete; previous gauges retained"
                );
            }
            self.source.page(after).await
        })
    }
}
