//! A captured reply cannot outlive its native authority or process assignment.
use super::*;

impl AgentSandboxBackend {
    pub(super) async fn exchange_bound_status(
        &self,
        handle: &RuntimeHandle,
        request: RuntimeStatusRequest,
    ) -> Result<ResponseFrame> {
        let assigned = self.assignments.lock().unwrap().get(handle).cloned();
        if let Some(assigned) = &assigned {
            let identity = crate::hosted::provider_request_identity(&assigned.request)?;
            if request.sandbox_id != identity.sandbox_id
                || request.generation != identity.generation
                || assigned.request.metadata.get("cma/home-identity")
                    != Some(&request.home_identity)
            {
                return Err(AdapterError::Rejected(
                    "status request differs from its acknowledged assignment".into(),
                ));
            }
        } else {
            // Eviction or a missing incarnation requires adoption/bootstrap to
            // supply a new acknowledgement; it must not bypass native proof.
            let object = self
                .client
                .get(handle)
                .await?
                .ok_or_else(|| AdapterError::Unavailable("status runtime is absent".into()))?;
            if object
                .annotations()
                .contains_key(crate::agent_sandbox::NATIVE_HOME_ANNOTATION)
            {
                return Err(AdapterError::Unavailable(
                    "native status requires a fresh acknowledged assignment".into(),
                ));
            }
        }
        let generation = request.generation;
        let exchange = self
            .transport
            .exchange(handle, RequestFrame::RuntimeStatus(request));
        let response = match &assigned {
            Some(assigned) => {
                crate::agent_sandbox::relay_readiness_retry::bind(
                    handle,
                    &assigned.pod,
                    &assigned.request,
                    generation,
                    &assigned.request_sha256,
                    exchange,
                )
                .await?
            }
            None => exchange.await?,
        };
        if let Some(assigned) = assigned {
            crate::file_read_diagnostics::measure(
                crate::file_read_diagnostics::Stage::StatusPostValidation,
                timed(Stage::NativeIdentity, async {
                    let object = self
                        .client
                        .validate_request(handle, &assigned.request)
                        .await?
                        .ok_or_else(|| {
                            AdapterError::Unavailable("status runtime disappeared".into())
                        })?;
                    self.client.verify_explicit_runtime_policy(&object).await?;
                    let pod = self.client.pod_binding(handle).await?;
                    // Bootstrap can replay the same assignment while status is in flight.
                    // Compare its authority after the fresh Pod and policy reads.
                    if pod != assigned.pod
                        || self.assignments.lock().unwrap().get(handle) != Some(&assigned)
                    {
                        return Err(AdapterError::Rejected(
                            "native status assignment changed during exchange".into(),
                        ));
                    }
                    Ok(())
                }),
            )
            .await?;
        }
        Ok(response)
    }
    pub(super) async fn exchange_frame(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> Result<ResponseFrame> {
        if let RequestFrame::RuntimeStatus(status) = request {
            return self.exchange_bound_status(handle, status).await;
        }
        self.transport.exchange(handle, request).await
    }

    pub(super) fn exchange_admitted_frame<'a>(
        &'a self,
        handle: &'a RuntimeHandle,
        request: OwnedWork<RequestFrame>,
        policy: &IngressOwnershipPolicy,
    ) -> std::result::Result<AdmittedOperation<'a, ResponseFrame>, AdmittedError> {
        if !request.belongs_to(policy) {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "native exchange request ownership",
            }
            .into());
        }
        if matches!(&*request, RequestFrame::RuntimeStatus(_)) {
            // Status must retain exchange_bound_status's assignment and Pod
            // checks. Its native reads do not yet have admitted ports.
            return Err(AdmittedError::Unsupported(
                "native status validation requires admitted reads",
            ));
        }
        self.transport.exchange_admitted(handle, request, policy)
    }
}
