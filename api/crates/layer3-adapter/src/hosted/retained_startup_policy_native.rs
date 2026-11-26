use super::AgentSandboxBackend;
use crate::{provider::ProviderSandboxRequest, Result, RuntimeHandle};

#[async_trait::async_trait]
impl crate::hosted::HostedRetainedStartupPolicy for AgentSandboxBackend {
    async fn inspect_retained_startup_policy(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &crate::HomeVolumeAttachment,
    ) -> Result<crate::RetainedStartupPolicyPredecessor> {
        self.client
            .observe_retained_startup_policy_predecessor(handle, request, home)
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
        self.client
            .upgrade_retained_startup_policy(handle, previous, home, admission)
            .await?;
        if let Some(until) = next.funded_until {
            self.client
                .set_runtime_deadline(handle, next, until)
                .await?;
        }
        self.bind_resumed(handle, next).await
    }

    async fn verify_retained_startup_policy(
        &self,
        handle: &RuntimeHandle,
        readback: &crate::RetainedStartupPolicyReadback<'_>,
    ) -> Result<crate::RuntimeStartupPolicyObservation> {
        self.transport
            .verify_retained_startup_policy(handle, readback)
            .await
    }
}
