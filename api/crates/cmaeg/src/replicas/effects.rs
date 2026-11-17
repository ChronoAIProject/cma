//! Typed handoff for delayed tool/media work in the original conversation FIFO.
use super::{
    partition::{PartitionLease, Partitions},
    partition_model::{item_id, Item},
    Error,
};
use crate::{
    ingress::routing_store::{AdmittedPlan, PlanStore, StoredTarget},
    turns::routed::{
        scheduler::{Disposition, TargetExecutor, WorkItem},
        ExecutionError,
    },
};
use futures_util::future::BoxFuture;
use std::{sync::Arc, time::Duration};

pub enum EffectDisposition {
    Completed,
    Retained,
    Retry,
}

/// Implementations retain immutable payloads before enqueueing. A recovered
/// sending/unknown marker never authorizes a second external attempt.
pub trait EffectExecutor: Send + Sync {
    fn execute<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        work_id: &'a str,
        token: &'a mut PartitionLease,
    ) -> BoxFuture<'a, Result<EffectDisposition, ExecutionError>>;
    /// Repair one bounded page of durable intents whose queue acknowledgement
    /// may have been lost. The cursor must advance past completed roots too.
    fn recover<'a>(
        &'a self,
        after: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Option<String>, ExecutionError>>;
}

pub(crate) struct Executor {
    pub partitions: Partitions,
    pub plans: PlanStore,
    pub targets: Arc<dyn TargetExecutor>,
    pub effects: Arc<dyn EffectExecutor>,
}
impl TargetExecutor for Executor {
    fn execute<'a>(&'a self, work: &'a WorkItem) -> BoxFuture<'a, Disposition> {
        Box::pin(async move {
            match self.run(work).await {
                Ok(disposition) => disposition,
                Err(error) => {
                    tracing::warn!(
                        ?error,
                        "replica effect scheduling deferred; retaining FIFO custody"
                    );
                    Disposition::Retry(Duration::from_millis(250))
                }
            }
        })
    }
}
impl Executor {
    async fn run(&self, work: &WorkItem) -> Result<Disposition, ExecutionError> {
        let (item, _) = self
            .partitions
            .store
            .read::<Item>(&item_id(&work.target_id), true)
            .await?
            .ok_or(Error::Lost)?;
        if item.work_id != work.target_id || item.identity != work.identity {
            return Err(ExecutionError::Corrupt);
        }
        if item.done {
            return Ok(Disposition::Settled);
        }
        if !item.effect {
            return Ok(self.targets.execute(work).await);
        }
        let plan = self
            .plans
            .load(&work.identity)
            .await
            .map_err(|_| ExecutionError::Unavailable)?
            .ok_or(ExecutionError::Corrupt)?;
        let target = plan
            .targets
            .iter()
            .find(|target| target.id == item.target)
            .ok_or(ExecutionError::Corrupt)?;
        if item.request_hash != plan.request_hash {
            return Err(ExecutionError::Corrupt);
        }
        let mut token = self
            .partitions
            .claim_work(&plan, target, &item.work_id)
            .await?;
        let result = self
            .effects
            .execute(&plan, target, &item.work_id, &mut token)
            .await;
        match result {
            Ok(EffectDisposition::Completed) => {
                self.partitions.complete(&token).await?;
                Ok(Disposition::Settled)
            }
            result => {
                if self.partitions.release(&token).await.is_err() {
                    tracing::warn!("replica effect release failed; preserving lease until expiry");
                }
                match result {
                    Ok(EffectDisposition::Retained) => Ok(Disposition::Settled),
                    Ok(EffectDisposition::Retry) => {
                        Ok(Disposition::Retry(Duration::from_millis(250)))
                    }
                    Err(error) => Err(error),
                    Ok(EffectDisposition::Completed) => unreachable!(),
                }
            }
        }
    }
}
