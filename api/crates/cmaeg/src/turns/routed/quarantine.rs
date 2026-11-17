//! Target-wide reply revocation; execution and delivery evidence remain unchanged.
use super::{storage, ExecutionError, Journal, RoutedToolReplies};
use crate::ingress::routing_store::{AdmittedPlan, StoredTarget};
use serde::{Deserialize, Serialize};

pub(crate) const KIND: &str = "replica_target_quarantine_v1";
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Quarantine {
    pub kind: String,
    pub target: String,
    pub root: String,
    pub owner: String,
    pub request_hash: String,
    pub retained_until: i64,
    pub audit: String,
    pub rollback_safe_after: i64,
}
pub(crate) fn id(target: &str) -> String {
    storage::identifier(&(KIND, target))
}
impl Journal {
    pub(crate) async fn quarantined(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<bool, ExecutionError> {
        let Some((value, _)) = self.read::<Quarantine>(&id(&target.id)).await? else {
            return Ok(false);
        };
        if value.kind != KIND
            || value.target != target.id
            || value.root != plan.admission_id
            || value.owner != plan.identity.owner
            || value.request_hash != plan.request_hash
            || value.retained_until != plan.retained_until
            || value.rollback_safe_after
                != target
                    .launch
                    .event
                    .expires_at
                    .saturating_add(cmaeg_core::seal::REPLY_TARGET_TTL)
        {
            return Err(ExecutionError::Corrupt);
        }
        Ok(true)
    }
    pub(crate) async fn require_reply_open(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<(), ExecutionError> {
        if self.quarantined(plan, target).await? {
            Err(ExecutionError::Abandoned)
        } else {
            Ok(())
        }
    }
}
impl RoutedToolReplies {
    pub async fn authorize_reply_target_mint(
        &self,
        event: &cmaeg_core::seal::EventRefPayload,
    ) -> Result<(), ExecutionError> {
        let Some(binding) = self.journal.tool_reply_binding(event).await? else {
            return Ok(());
        };
        let plan = self
            .plans
            .load(&binding.identity)
            .await
            .map_err(super::dispatcher::admission_error)?
            .ok_or(ExecutionError::Corrupt)?;
        let target = plan
            .targets
            .iter()
            .find(|t| t.id == binding.target)
            .ok_or(ExecutionError::Corrupt)?;
        self.journal.require_reply_open(&plan, target).await
    }
}
