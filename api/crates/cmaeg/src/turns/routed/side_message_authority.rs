//! Read-only routed authority for side messages, independent of recovery workers.
use super::{
    dispatcher::{admission_error, current_target},
    model::delivery_event,
    tool_replies::same_event,
    Authority, ExecutionError, Journal,
};
use crate::{auth::AgentCaller, ingress::routing_store::PlanStore, records::RecordClient};
use cmaeg_core::seal::{EventRefPayload, MasterKeyRing};
use std::sync::Arc;

pub(crate) struct SideMessageAuthority {
    plans: PlanStore,
    journal: Journal,
    authority: Arc<dyn Authority>,
    require_binding: bool,
}

impl SideMessageAuthority {
    pub(crate) fn new(
        records: RecordClient,
        keys: Arc<MasterKeyRing>,
        authority: Arc<dyn Authority>,
        require_binding: bool,
    ) -> Self {
        Self {
            plans: PlanStore::new(records.clone(), keys.clone()),
            journal: Journal::new(records, keys),
            authority,
            require_binding,
        }
    }

    /// Recheck retained target authority independently of the answer ledger.
    pub(crate) async fn check(
        &self,
        caller: AgentCaller,
        event: &EventRefPayload,
        channel_version: u64,
    ) -> Result<(), ExecutionError> {
        let Some(binding) = self.journal.tool_reply_binding(event).await? else {
            return if self.require_binding {
                Err(ExecutionError::Unsupported)
            } else {
                Ok(())
            };
        };
        let plan = self
            .plans
            .load(&binding.identity)
            .await
            .map_err(admission_error)?
            .ok_or(ExecutionError::Corrupt)?;
        if plan.admission_id != binding.root || plan.request_hash != binding.request_hash {
            return Err(ExecutionError::Corrupt);
        }
        let target = plan
            .targets
            .iter()
            .find(|target| target.id == binding.target)
            .ok_or(ExecutionError::Corrupt)?;
        if !same_event(&delivery_event(&plan, target), event, false) {
            return Err(ExecutionError::AuthorityChanged);
        }
        let current = current_target(self.authority.as_ref(), &plan, target).await?;
        if current.version != channel_version
            || caller.consumer(&current).ok().as_ref() != Some(&target.launch.consumer)
        {
            return Err(ExecutionError::AuthorityChanged);
        }
        self.journal.require_reply_open(&plan, target).await
    }
}
