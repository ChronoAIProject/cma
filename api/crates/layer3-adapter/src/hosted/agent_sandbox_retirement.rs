use super::*;

impl AgentSandboxBackend {
    pub(super) async fn challenge_receiver(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
        receiver: &crate::receiver_failure::RetirementReceiver,
    ) -> Result<bool> {
        if receiver.pod.is_uid_only() {
            return self
                .transport
                .probe_unreachable_receiver(
                    handle,
                    request,
                    home,
                    &receiver.owner,
                    Some(&receiver.pod.uid),
                )
                .await
                .map(|observed| observed.as_ref() == Some(&receiver.pod));
        }
        let binding = self
            .client
            .retirement_receiver(handle, request, home)
            .await?;
        if RetirementPod::try_from(&binding)? != receiver.pod {
            return Err(AdapterError::Rejected("retirement receiver changed".into()));
        }
        let unavailable = self
            .transport
            .challenge_retirement_receiver(handle, &binding, &receiver.owner)
            .await?;
        if self
            .client
            .retirement_receiver(handle, request, home)
            .await?
            != binding
        {
            return Err(AdapterError::Rejected("retirement receiver changed".into()));
        }
        Ok(unavailable)
    }
    pub(super) async fn retire_with_proof(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
        receiver: Option<&RetirementPod>,
    ) -> Result<()> {
        if let Some(receiver) = receiver {
            self.client
                .retire_verified(handle, request, home, Some(receiver))
                .await?;
        } else {
            self.client.retire_suspended(handle, request, home).await?;
        }
        self.forget_assignment(handle);
        self.transport.retire(handle).await
    }
}
