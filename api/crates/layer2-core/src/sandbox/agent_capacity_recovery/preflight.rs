//! Authorized metadata-only dry-run. No log access, admission or runtime calls.
use std::sync::Arc;

use serde::Serialize;

use super::{CapacityRecoveryContext, CapacityRecoveryDisposition};
use crate::resource_access::{
    AccessPurpose, AuthorizedResource, CallerContext, ResourceAccessError, ResourceAccessService,
    ResourceAccessStoreError, ResourceCapability, ResourceTarget,
};
use crate::{AgentId, Caller};

/// This observation never claims certified source evidence or recovery success.
/// The original request and runtime/home/input identities stay internal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapacityRecoveryPreflight {
    pub agent_id: AgentId,
    pub disposition: CapacityRecoveryDisposition,
    pub observed_revision: u64,
    pub through_seq: u64,
    pub evaluated_at: String,
}

pub struct CapacityRecoveryPreflightService {
    access: Arc<ResourceAccessService>,
}

impl CapacityRecoveryPreflightService {
    pub fn new(access: Arc<ResourceAccessService>) -> Self {
        Self { access }
    }

    /// Disabled/missing admission gates still permit an authorized dry-run.
    pub async fn read(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<CapacityRecoveryPreflight, ResourceAccessError> {
        let target = ResourceTarget::Agent {
            agent_id: id.clone(),
        };
        let authorized = self
            .access
            .authorize_personal(caller, &target, ResourceCapability::OperateAgent)
            .await?;
        self.finish(authorized, target).await
    }

    pub async fn read_authorized(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentId,
    ) -> Result<CapacityRecoveryPreflight, ResourceAccessError> {
        let target = ResourceTarget::Agent {
            agent_id: id.clone(),
        };
        let authorized = self
            .access
            .authorize(
                context,
                credential,
                &target,
                ResourceCapability::OperateAgent,
                AccessPurpose::Read,
            )
            .await?;
        self.finish(authorized, target).await
    }

    async fn finish(
        &self,
        authorized: AuthorizedResource,
        target: ResourceTarget,
    ) -> Result<CapacityRecoveryPreflight, ResourceAccessError> {
        // Recheck current policy/binding before release and classify this latest
        // coherent ancestry. Parent retirement after authorization must be seen.
        let current = self
            .access
            .current_read_snapshot(&authorized, &target, ResourceCapability::OperateAgent)
            .await?;
        let lineage = current.lineage;
        let context = CapacityRecoveryContext {
            agent: lineage
                .agent
                .as_ref()
                .ok_or(ResourceAccessStoreError::Corrupt)?,
            sandbox: &lineage.sandbox,
            workspace: lineage
                .workspace
                .as_ref()
                .ok_or(ResourceAccessStoreError::Corrupt)?,
            worktree: lineage
                .worktree
                .as_ref()
                .ok_or(ResourceAccessStoreError::Corrupt)?,
        };
        let now = crate::now_rfc3339();
        let result = CapacityRecoveryPreflight {
            agent_id: context.agent.id.clone(),
            disposition: context.classify(&now),
            observed_revision: context.agent.revision,
            through_seq: context.agent.conversation.last_seq,
            evaluated_at: now,
        };
        authorized
            .decision
            .require(ResourceCapability::OperateAgent)?;
        Ok(result)
    }
}
