//! Durable scheduling is part of acceptance when replica ownership is enabled.
use super::{AdmissionError, AdmittedPlan, PlanStore};
use futures_util::future::BoxFuture;
use std::sync::Arc;

/// The ownership implementation assigns each target a persistent FIFO position.
/// Re-entry must preserve that position, including completed target tombstones.
/// A partial enqueue is recoverable from the immutable admission root and must
/// return an error until every target has durable scheduling custody.
pub trait AdmissionQueue: Send + Sync {
    fn enqueue<'a>(&'a self, plan: &'a AdmittedPlan) -> BoxFuture<'a, Result<(), AdmissionError>>;
}

impl PlanStore {
    /// Install only with the coordinated replica writer switch. Absence keeps
    /// the existing admission format and scheduler behavior unchanged.
    pub fn with_admission_queue(mut self, queue: Arc<dyn AdmissionQueue>) -> Self {
        self.admission_queue = Some(queue);
        self
    }

    pub(super) async fn queue_committed(
        &self,
        plan: AdmittedPlan,
    ) -> Result<AdmittedPlan, AdmissionError> {
        if let Some(queue) = &self.admission_queue {
            let result = queue.enqueue(&plan).await;
            metrics::counter!(
                "cmaeg_replica_admission_queue_total",
                "outcome" => if result.is_ok() { "ready" } else { "unavailable" }
            )
            .increment(1);
            if let Err(error) = result {
                // The root remains durable. A caller retries its original event;
                // recovery scanning also repairs the root-to-queue crash window.
                tracing::warn!(
                    admission_sha256 = super::codec::hash(plan.admission_id.as_bytes()),
                    ?error,
                    "Durable replica scheduling incomplete; retaining admission"
                );
                return Err(error);
            }
        }
        Ok(plan)
    }
}
