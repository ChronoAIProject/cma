//! Shared seam for execution, automatic delivery and independently fenced tools.
use super::{ExecutionError, Journal};
use crate::{
    ingress::routing_store::{AdmittedPlan, StoredTarget},
    replicas::partition::{PartitionLease, Partitions},
};
use serde::Serialize;

impl Journal {
    pub(crate) fn with_partitions(mut self, partitions: Partitions) -> Self {
        self.clock = partitions.clock();
        self.partitions = Some(partitions);
        self
    }

    pub fn effect_executor(
        &self,
        plans: crate::ingress::routing_store::PlanStore,
        targets: std::sync::Arc<dyn super::scheduler::TargetExecutor>,
        effects: std::sync::Arc<dyn crate::replicas::effects::EffectExecutor>,
    ) -> Result<std::sync::Arc<dyn super::scheduler::TargetExecutor>, ExecutionError> {
        let partitions = self.partitions.clone().ok_or(ExecutionError::Unsupported)?;
        Ok(std::sync::Arc::new(crate::replicas::effects::Executor {
            partitions,
            plans,
            targets,
            effects,
        }))
    }

    pub async fn enqueue_effect(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        work_id: &str,
    ) -> Result<(), ExecutionError> {
        self.partitions
            .as_ref()
            .ok_or(ExecutionError::Unsupported)?
            .enqueue_effect(plan, target, work_id)
            .await
            .map_err(Into::into)
    }

    pub async fn partition_owner(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<Option<crate::replicas::partition::PartitionOwner>, ExecutionError> {
        match &self.partitions {
            Some(partitions) => Ok(Some(partitions.owner(plan, target).await?)),
            None => Ok(None),
        }
    }

    /// Capture from this process's live owner only. A request on another replica
    /// needs durable owner routing; a lookup must never adopt its owner's token.
    pub async fn capture_partition(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<Option<PartitionLease>, ExecutionError> {
        match &self.partitions {
            Some(partitions) => Ok(Some(partitions.capture(plan, target).await?)),
            None => Ok(None),
        }
    }

    pub async fn guard_partition(
        &self,
        token: &Option<PartitionLease>,
    ) -> Result<(), ExecutionError> {
        match (&self.partitions, token) {
            (Some(partitions), Some(token)) => partitions
                .guard(token)
                .await
                .map(|_| ())
                .map_err(Into::into),
            (None, None) => Ok(()),
            _ => Err(ExecutionError::LostLease),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn write_fenced(
        &self,
        token: &Option<PartitionLease>,
        id: &str,
        owner: &str,
        retained_until: i64,
        value: &impl Serialize,
        expected: Option<u64>,
    ) -> Result<u64, ExecutionError> {
        match (&self.partitions, token) {
            (None, None) => self.write(id, owner, retained_until, value, expected).await,
            (Some(partitions), Some(token)) => partitions
                .write(token, id, owner, retained_until, value, expected)
                .await
                .map_err(Into::into),
            _ => Err(ExecutionError::LostLease),
        }
    }
}
