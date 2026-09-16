//! A Workspace job claim is distinct from the original completed child job.
use super::reuse_cleanup::ChannelReuseCleanupAuthority;
use layer2_core::bridge::{allocation::AllocationError, cleanup::WorkspaceCleanupJobClaim};

impl super::BridgeProvisioningAuthority {
    pub(crate) async fn workspace_cleanup_support(
        &self,
        sandbox: &layer2_core::sandbox::Sandbox,
        deployment_id: &str,
    ) -> Result<layer3_adapter::WorkspaceRemovalSupport, AllocationError> {
        let handle = crate::sandbox::runtime_attachment::adopt_bound_runtime(
            self.dependencies.adapter.as_ref(),
            sandbox,
            deployment_id,
        )
        .await
        .map_err(|_| AllocationError::Unavailable)?;
        let support = self
            .dependencies
            .adapter
            .workspace_removal_support(&handle)
            .await
            .map_err(|_| AllocationError::Unavailable)?;
        if support.protocol_version != layer3_adapter::WORKSPACE_REMOVAL_PROTOCOL
            || !sandbox
                .runtime_process_matches(&support.runtime)
                .map_err(|_| AllocationError::Unavailable)?
            || !support.runtime.installation_ready
            || !support.runtime.credential_ready
            || support.runtime.restore_pending
        {
            return Err(AllocationError::Unavailable);
        }
        Ok(support)
    }
}

pub(crate) struct WorkspaceCleanupAuthority {
    common: ChannelReuseCleanupAuthority,
    id: String,
    claim: WorkspaceCleanupJobClaim,
}
impl WorkspaceCleanupAuthority {
    pub(crate) fn bind(
        mut common: ChannelReuseCleanupAuthority,
        id: &str,
        claim: &WorkspaceCleanupJobClaim,
    ) -> Result<Self, AllocationError> {
        let now = mongodb::bson::DateTime::now().timestamp_millis();
        let remaining = u64::try_from(claim.expires_at_ms - now)
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        if remaining == 0 || common.job().is_some() {
            return Err(AllocationError::AuthorityUnavailable);
        }
        common.bound_deadline(
            tokio::time::Instant::now() + std::time::Duration::from_millis(remaining),
        );
        let value = Self {
            common,
            id: id.into(),
            claim: claim.clone(),
        };
        value.require_current()?;
        Ok(value)
    }
    pub(crate) fn common(&self) -> &ChannelReuseCleanupAuthority {
        &self.common
    }
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
    pub(crate) fn claim(&self) -> &WorkspaceCleanupJobClaim {
        &self.claim
    }
    pub(crate) fn require_current(&self) -> Result<(), AllocationError> {
        self.common.require_current()?;
        if mongodb::bson::DateTime::now().timestamp_millis() >= self.claim.expires_at_ms {
            return Err(AllocationError::AuthorityUnavailable);
        }
        Ok(())
    }
}
