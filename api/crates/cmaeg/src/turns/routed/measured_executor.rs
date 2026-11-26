use super::scheduler::{Disposition, TargetExecutor, WorkItem};
use crate::telemetry::provider_start::Measurement;
use futures_util::future::BoxFuture;
use std::sync::Arc;

pub(super) struct MeasuredExecutor {
    pub executor: Arc<dyn TargetExecutor>,
    pub measurement: Measurement,
}
impl TargetExecutor for MeasuredExecutor {
    fn execute<'a>(&'a self, work: &'a WorkItem) -> BoxFuture<'a, Disposition> {
        Box::pin(async move {
            let mut observation = self.measurement.executor(&work.identity.admission_id());
            let result = self.executor.execute(work).await;
            observation.finished();
            result
        })
    }
}
