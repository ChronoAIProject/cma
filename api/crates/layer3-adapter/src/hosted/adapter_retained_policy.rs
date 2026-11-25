use super::HostedAdapter;
use crate::hosted::retained_startup_policy::require;
use crate::{Result, RuntimeAdoptionRequest, RuntimeHandle, RuntimeProvisionRequest};

#[async_trait::async_trait]
impl crate::RetainedStartupPolicyControl for HostedAdapter {
    async fn inspect_retained_startup_policy(
        &self,
        request: &RuntimeAdoptionRequest,
    ) -> Result<crate::RetainedStartupPolicyPredecessor> {
        self.inspect_retained_exact(request).await
    }

    async fn upgrade_retained_runtime_policy(
        &self,
        request: &RuntimeAdoptionRequest,
        next: &RuntimeProvisionRequest,
        admission: &crate::RetainedStartupPolicyUpgrade<'_>,
    ) -> Result<RuntimeHandle> {
        self.upgrade_retained_exact(request, next, admission).await
    }

    async fn verify_retained_startup_policy(
        &self,
        handle: &RuntimeHandle,
        readback: &crate::RetainedStartupPolicyReadback<'_>,
    ) -> Result<crate::RuntimeStartupPolicyObservation> {
        require(self.backend.as_ref())?
            .verify_retained_startup_policy(handle, readback)
            .await
    }
}
