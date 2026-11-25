use super::*;

#[async_trait::async_trait]
impl crate::hosted::HostedRetainedStartupPolicy for RuntimeBackends {
    async fn inspect_retained_startup_policy(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &crate::HomeVolumeAttachment,
    ) -> Result<crate::RetainedStartupPolicyPredecessor> {
        crate::hosted::retained_startup_policy::require(self.for_handle(handle)?)?
            .inspect_retained_startup_policy(handle, request, home)
            .await
    }

    async fn upgrade_retained_policy_with_home(
        &self,
        handle: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
        home: &crate::HomeVolumeAttachment,
        admission: &crate::RetainedStartupPolicyUpgrade<'_>,
    ) -> Result<()> {
        crate::hosted::retained_startup_policy::require(self.for_handle(handle)?)?
            .upgrade_retained_policy_with_home(handle, previous, next, home, admission)
            .await
    }

    async fn verify_retained_startup_policy(
        &self,
        handle: &RuntimeHandle,
        readback: &crate::RetainedStartupPolicyReadback<'_>,
    ) -> Result<crate::RuntimeStartupPolicyObservation> {
        crate::hosted::retained_startup_policy::require(self.for_handle(handle)?)?
            .verify_retained_startup_policy(handle, readback)
            .await
    }
}
