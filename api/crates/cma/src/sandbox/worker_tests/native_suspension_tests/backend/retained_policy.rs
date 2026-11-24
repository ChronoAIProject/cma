use super::*;

#[async_trait::async_trait]
impl layer3_adapter::hosted::HostedRetainedStartupPolicy for NativeBackend {
    async fn inspect_retained_startup_policy(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &layer3_adapter::HomeVolumeAttachment,
    ) -> layer3_adapter::Result<layer3_adapter::RetainedStartupPolicyPredecessor> {
        self.calls.push("inspect-retained-predecessor");
        let rows = self.row.lock().await;
        let row = rows.as_ref().ok_or_else(absent)?;
        if row.observation.handle != *handle || row.request != *request || row.pod.is_some() {
            return Err(absent());
        }
        Ok(layer3_adapter::RetainedStartupPolicyPredecessor {
            uid: home
                .receipt
                .native_authority
                .as_ref()
                .unwrap()
                .sandbox_uid
                .clone(),
            resource_version: self.proof_revision.load(Ordering::SeqCst).to_string(),
            request_sha256: layer3_adapter::provider_request_digest(request)?,
            home_identity: home.identity()?,
            image: request.image.uri.clone(),
            generation: row.observation.generation,
            journal_recovery: false,
        })
    }

    async fn upgrade_retained_policy_with_home(
        &self,
        handle: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
        _: &layer3_adapter::HomeVolumeAttachment,
        admission: &layer3_adapter::RetainedStartupPolicyUpgrade<'_>,
    ) -> layer3_adapter::Result<()> {
        let mut expected = previous.retained_startup_policy_successor(
            admission.target_image,
            admission.successor_generation,
            admission.journal_recovery,
        )?;
        expected.funded_until = next.funded_until;
        assert_eq!(&expected, next);
        self.calls.push("upgrade-retained");
        self.resume(handle, previous, next).await
    }

    async fn verify_retained_startup_policy(
        &self,
        _: &RuntimeHandle,
        _: &layer3_adapter::RetainedStartupPolicyReadback<'_>,
    ) -> layer3_adapter::Result<layer3_adapter::RuntimeStartupPolicyObservation> {
        Err(absent())
    }
}
