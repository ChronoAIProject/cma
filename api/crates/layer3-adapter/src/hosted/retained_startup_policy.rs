use crate::{provider::ProviderSandboxRequest, Result, RuntimeHandle};

/// Provider operations for the admitted retained-home transition.
#[async_trait::async_trait]
pub trait HostedRetainedStartupPolicy: Send + Sync {
    async fn inspect_retained_startup_policy(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &crate::HomeVolumeAttachment,
    ) -> Result<crate::RetainedStartupPolicyPredecessor>;

    async fn upgrade_retained_policy_with_home(
        &self,
        handle: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
        home: &crate::HomeVolumeAttachment,
        admission: &crate::RetainedStartupPolicyUpgrade<'_>,
    ) -> Result<()>;

    async fn verify_retained_startup_policy(
        &self,
        handle: &RuntimeHandle,
        readback: &crate::RetainedStartupPolicyReadback<'_>,
    ) -> Result<crate::RuntimeStartupPolicyObservation>;
}

pub(super) fn require(
    backend: &dyn super::HostedBackend,
) -> Result<&dyn HostedRetainedStartupPolicy> {
    backend.retained_startup_policy().ok_or_else(|| {
        crate::AdapterError::Unsupported("retained runtime startup policy is unavailable".into())
    })
}
