//! Effect custody and discovery share the partition transaction boundary.
use super::{ExecutionError, Journal};
use crate::replicas::partition::PartitionLease;
use serde::Serialize;

impl Journal {
    pub(super) async fn admit_delayed_root(
        &self,
        plan: &crate::ingress::routing_store::AdmittedPlan,
        target: &crate::ingress::routing_store::StoredTarget,
        work: &str,
        value: &impl Serialize,
    ) -> Result<(), ExecutionError> {
        self.partitions
            .as_ref()
            .ok_or(ExecutionError::Unsupported)?
            .admit_delayed(plan, target, work, value)
            .await
            .map_err(Into::into)
    }
    pub(super) async fn ensure_delayed_custody(
        &self,
        plan: &crate::ingress::routing_store::AdmittedPlan,
        target: &crate::ingress::routing_store::StoredTarget,
        work: &str,
    ) -> Result<(), ExecutionError> {
        self.partitions
            .as_ref()
            .ok_or(ExecutionError::Unsupported)?
            .ensure_delayed(plan, target, work)
            .await
            .map_err(Into::into)
    }

    pub async fn write_effect_root(
        &self,
        id: &str,
        owner: &str,
        retained_until: i64,
        value: &impl Serialize,
    ) -> Result<u64, ExecutionError> {
        self.partitions
            .as_ref()
            .ok_or(ExecutionError::Unsupported)?
            .write_effect_root(id, owner, retained_until, value)
            .await
            .map_err(Into::into)
    }
    pub async fn effect_roots(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(String, serde_json::Value, u64)>, ExecutionError> {
        self.partitions
            .as_ref()
            .ok_or(ExecutionError::Unsupported)?
            .effect_roots(after, limit)
            .await
            .map_err(Into::into)
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn write_effect_fenced(
        &self,
        token: &Option<PartitionLease>,
        id: &str,
        owner: &str,
        retained_until: i64,
        value: &impl Serialize,
        expected: Option<u64>,
        active: bool,
    ) -> Result<u64, ExecutionError> {
        let token = token.as_ref().ok_or(ExecutionError::LostLease)?;
        self.partitions
            .as_ref()
            .ok_or(ExecutionError::Unsupported)?
            .write_effect_fenced(token, id, owner, retained_until, value, expected, active)
            .await
            .map_err(Into::into)
    }
}
