//! Exact finality checks shared by the hosted adapter lifecycle dispatch.
use super::*;

impl HostedAdapter {
    pub(super) async fn finalize_runtime_exact(
        &self,
        original: &crate::RuntimeProvisionReceipt,
        operation_id: &str,
    ) -> crate::Result<crate::RuntimeFinalityReceipt> {
        let receipt = self
            .backend
            .finalize_runtime(original, operation_id)
            .await?;
        receipt.validate()?;
        if receipt.original() != original || receipt.delete_operation_id() != operation_id {
            return Err(AdapterError::Rejected(
                "provider returned another runtime finality receipt".into(),
            ));
        }
        self.states.lock().await.remove(original.handle());
        Ok(receipt)
    }

    pub(super) async fn retire_unreachable_exact(
        &self,
        request: &crate::RuntimeAdoptionRequest,
        receiver: &crate::RetirementPod,
    ) -> crate::Result<()> {
        let rendered = self.retained_request(request, request.generation)?;
        self.verify_adoption_home_route(request)?;
        let home = request.spec.home_volume.as_ref().ok_or_else(|| {
            AdapterError::Rejected("retirement requires the original home".into())
        })?;
        home.validate_for(&request.spec.sandbox_id)?;
        self.backend
            .retire_unreachable(&request.handle, &rendered, home, receiver)
            .await?;
        self.states.lock().await.remove(&request.handle);
        Ok(())
    }

    pub(super) async fn verify_retired_exact(
        &self,
        request: &crate::RuntimeAdoptionRequest,
    ) -> crate::Result<()> {
        self.verify_adoption_home_route(request)?;
        let home = request.spec.home_volume.as_ref().ok_or_else(|| {
            AdapterError::Rejected("retirement requires the original home".into())
        })?;
        home.validate_for(&request.spec.sandbox_id)?;
        self.backend.verify_retired(&request.handle, home).await
    }

    pub(super) async fn teardown_retained_exact(
        &self,
        handle: &RuntimeHandle,
    ) -> crate::Result<()> {
        self.backend.teardown_retained(handle).await?;
        self.states.lock().await.remove(handle);
        Ok(())
    }
}

#[async_trait::async_trait]
impl crate::RuntimeRetirement for HostedAdapter {
    async fn challenge_retirement_receiver(
        &self,
        request: &crate::RuntimeAdoptionRequest,
        receiver: &crate::receiver_failure::RetirementReceiver,
    ) -> crate::Result<bool> {
        let rendered = self.retained_request(request, request.generation)?;
        self.verify_adoption_home_route(request)?;
        if receiver.runtime != request.handle {
            return Err(AdapterError::Rejected(
                "retirement receiver belongs to another runtime".into(),
            ));
        }
        let home = request.spec.home_volume.as_ref().ok_or_else(|| {
            AdapterError::Rejected("retirement requires the original home".into())
        })?;
        self.backend
            .challenge_retirement_receiver(&request.handle, &rendered, home, receiver)
            .await
    }

    async fn retire_unreachable(
        &self,
        request: &crate::RuntimeAdoptionRequest,
        receiver: &crate::RetirementPod,
    ) -> crate::Result<()> {
        self.retire_unreachable_exact(request, receiver).await
    }
    async fn verify_retired(&self, request: &crate::RuntimeAdoptionRequest) -> crate::Result<()> {
        self.verify_retired_exact(request).await
    }
}
