use super::*;

impl NativeRetirementFinalizer {
    pub(super) async fn dispatch(
        &self,
        parent: &Sandbox,
        holder: &str,
        provider: &dyn RuntimeRetirement,
        request: &RuntimeAdoptionRequest,
        action: &mut RetirementAction,
        after_graceful_stop: bool,
    ) -> Result<Option<RetirementOutcome>, SandboxStoreError> {
        if !after_graceful_stop {
            match probe(provider, request, action).await {
                Some(true) => {}
                Some(false) if action.phase == RetirementPhase::Admitted => {
                    self.records.transition(parent, action, None).await?;
                    return Ok(Some(RetirementOutcome::GracefulPending));
                }
                Some(false) => {
                    // The previous provider call may still be in flight.
                    // Resume normal closes while retaining the same custody.
                    return Ok(Some(RetirementOutcome::GracefulWithCustody));
                }
                None => {
                    return Ok(Some(RetirementOutcome::ProviderPending(
                        "retirement receiver proof is unavailable",
                    )))
                }
            }
        }
        if action.phase == RetirementPhase::Admitted {
            let mut dispatched = action.clone();
            dispatched.phase = RetirementPhase::Dispatching;
            self.records
                .transition(parent, action, Some(&dispatched))
                .await?;
            *action = dispatched;
        }
        self.delete_with_custody(
            parent,
            holder,
            action,
            provider.retire_unreachable(request, &action.observation.receiver.pod),
        )
        .await
    }

    pub(super) async fn dispatch_suspended(
        &self,
        parent: &Sandbox,
        holder: &str,
        provider: &dyn RuntimeRetirement,
        request: &RuntimeAdoptionRequest,
        action: &RetirementAction,
    ) -> Result<Option<RetirementOutcome>, SandboxStoreError> {
        self.delete_with_custody(parent, holder, action, provider.retire_suspended(request))
            .await
    }

    async fn delete_with_custody(
        &self,
        parent: &Sandbox,
        holder: &str,
        action: &RetirementAction,
        delete: impl std::future::Future<Output = layer3_adapter::Result<()>>,
    ) -> Result<Option<RetirementOutcome>, SandboxStoreError> {
        if !self.current(parent, holder).await? {
            return Ok(Some(RetirementOutcome::Superseded));
        }
        if self.records.action(&parent.id).await?.as_ref() != Some(action) {
            return Err(SandboxStoreError::Conflict);
        }
        if !matches!(
            tokio::time::timeout(Duration::from_secs(180), delete).await,
            Ok(Ok(())),
        ) {
            return Ok(Some(RetirementOutcome::ProviderPending(
                "native retirement deletion remains unconfirmed",
            )));
        }
        Ok(None)
    }
}
